<#
.SYNOPSIS
  Boot the REAL vanilla Minecraft server headless for a given seed, force a
  square of chunks to generate, then collect the resulting Anvil region files.

.DESCRIPTION
  Deliverable (1) of the end-to-end .mca block-diff acceptance harness
  (docs/MCA_DIFF.md). It creates a throwaway server directory, accepts the EULA,
  writes a server.properties pinned to the requested seed with online-mode off,
  boots the jar, and — once the server prints "Done" — drives its console over
  stdin: `forceload add` a square around origin (which generates those chunks
  to FULL via a load ticket), `save-all flush`, then `stop`. Finally it copies
  the overworld region/*.mca into the output directory for the Rust differ.

  forceload caps a single command at 256 chunks, so the covered square is
  (2*Radius+1)^2 chunks centered on chunk (0,0); keep Radius <= 7.

  Two environment gotchas this script handles explicitly:
    * Windows PowerShell 5.1's redirected StandardInput prepends a UTF-8 BOM to
      the *first* byte written, which the server reads as a garbage command
      ("Unknown or incomplete command  ﻿<--[HERE]"). We send one throwaway
      blank line first to absorb that BOM, then write raw ASCII bytes.
    * MC 26.2 stores the overworld under world/dimensions/minecraft/overworld/,
      NOT the legacy world/region/.

.EXAMPLE
  powershell -File tools/mca_diff/run_vanilla.ps1 -Seed 1592639710 -OutDir C:\tmp\mca\1592639710
#>
param(
  [long]$Seed = 1592639710,
  [Parameter(Mandatory = $true)][string]$OutDir,
  [string]$Jar = "C:\Users\kiezi\mc-decompile\server.jar",
  [int]$Radius = 7,
  [string]$WorkDir = "",
  [int]$BootTimeoutSec = 240,
  [int]$SaveTimeoutSec = 600
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $Jar)) { throw "server jar not found: $Jar" }
$java = (Get-Command java -ErrorAction SilentlyContinue)
if ($null -eq $java) { throw "no JVM on PATH (java -version failed)" }

if ([string]::IsNullOrEmpty($WorkDir)) {
  $WorkDir = Join-Path $env:TEMP ("vela-vanilla-{0}-{1}" -f $Seed, [System.Guid]::NewGuid().ToString("N").Substring(0,8))
}
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
New-Item -ItemType Directory -Force -Path $OutDir  | Out-Null

$chunkCount = (2*$Radius+1)*(2*$Radius+1)
Write-Host "[runner] work dir : $WorkDir"
Write-Host "[runner] out  dir : $OutDir"
Write-Host "[runner] seed     : $Seed"
Write-Host "[runner] radius   : $Radius chunks ($chunkCount chunks)"

# --- EULA + server.properties -------------------------------------------------
Set-Content -Path (Join-Path $WorkDir "eula.txt") -Value "eula=true" -Encoding ascii

$props = @"
level-seed=$Seed
server-port=0
enable-rcon=false
enable-query=false
online-mode=false
level-type=minecraft:normal
gamemode=creative
spawn-protection=0
spawn-monsters=false
spawn-npcs=false
spawn-animals=false
generate-structures=true
allow-nether=false
view-distance=10
simulation-distance=10
max-players=1
sync-chunk-writes=true
enable-command-block=false
motd=vela-mca-diff
"@
Set-Content -Path (Join-Path $WorkDir "server.properties") -Value $props -Encoding ascii

# --- launch -------------------------------------------------------------------
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $java.Source
$psi.Arguments = "-Xmx2G -jar `"$Jar`" nogui"
$psi.WorkingDirectory = $WorkDir
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true

$proc = New-Object System.Diagnostics.Process
$proc.StartInfo = $psi
[void]$proc.Start()
# Drain stdout/stderr so the pipes never block; content also lands in logs/latest.log.
$proc.BeginOutputReadLine()
$proc.BeginErrorReadLine()

$log = Join-Path $WorkDir "logs\latest.log"
$stdinBase = $proc.StandardInput.BaseStream
function Send-Cmd([string]$cmd) {
  $bytes = [System.Text.Encoding]::ASCII.GetBytes($cmd + "`n")
  $stdinBase.Write($bytes, 0, $bytes.Length)
  $stdinBase.Flush()
}

function Wait-ForLog([string]$pattern, [int]$timeoutSec, [string]$what) {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  while ($true) {
    if ((Test-Path $log) -and (Select-String -Path $log -Pattern $pattern -Quiet)) { return }
    if ($proc.HasExited) { throw "server exited early while waiting for $what (see $log)" }
    if ($sw.Elapsed.TotalSeconds -gt $timeoutSec) { throw "timeout ($timeoutSec s) waiting for $what" }
    Start-Sleep -Milliseconds 300
  }
}

try {
  Write-Host "[runner] waiting for server boot ..."
  Wait-ForLog 'Done \(' $BootTimeoutSec "server 'Done'"
  Start-Sleep -Seconds 1

  # Absorb the leading-BOM garbage on its own throwaway line.
  Send-Cmd ""
  Start-Sleep -Milliseconds 500

  $b = $Radius * 16
  $x1 = -$b; $z1 = -$b; $x2 = ($b + 15); $z2 = ($b + 15)
  Write-Host "[runner] forceload add $x1 $z1 $x2 $z2"
  Send-Cmd "forceload add $x1 $z1 $x2 $z2"
  Wait-ForLog 'to be force loaded|forceloaded|Added .* chunk' 60 "forceload ack"

  # Chunks generate to FULL asynchronously over subsequent ticks. Wait
  # generously, scaled by count (roughly a chunk every few ms once warm).
  $genWait = [Math]::Max(15, [int]($chunkCount / 4))
  Write-Host "[runner] generating; waiting ${genWait}s ..."
  Start-Sleep -Seconds $genWait

  Write-Host "[runner] save-all flush ..."
  Send-Cmd "save-all flush"
  Wait-ForLog 'Saved the game' $SaveTimeoutSec "save-all flush"
  Start-Sleep -Seconds 2

  Write-Host "[runner] stopping server ..."
  Send-Cmd "stop"
  if (-not $proc.WaitForExit(120000)) { $proc.Kill(); throw "server did not stop within 120s" }
}
finally {
  if (-not $proc.HasExited) { try { $proc.Kill() } catch {} }
}

# --- collect ------------------------------------------------------------------
# MC 26.2 dimension layout: world/dimensions/minecraft/overworld/region/*.mca
$regionSrc = Join-Path $WorkDir "world\dimensions\minecraft\overworld\region"
if (-not (Test-Path $regionSrc)) {
  # Fall back to the legacy path just in case a future/older jar uses it.
  $legacy = Join-Path $WorkDir "world\region"
  if (Test-Path $legacy) { $regionSrc = $legacy } else { throw "no region dir at $regionSrc (see $log)" }
}
$mca = Get-ChildItem -Path $regionSrc -Filter "*.mca"
if ($mca.Count -eq 0) { throw "no .mca files produced (see $log)" }
foreach ($f in $mca) { Copy-Item $f.FullName (Join-Path $OutDir $f.Name) -Force }

Set-Content -Path (Join-Path $OutDir "SEED.txt") -Value "$Seed" -Encoding ascii
if (Test-Path $log) { Copy-Item $log (Join-Path $OutDir "server.log") -Force }

Write-Host "[runner] collected $($mca.Count) region file(s) to $OutDir"
Write-Host "[runner] done."
