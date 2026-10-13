param([Parameter(Mandatory=$true)][string]$OutputFile)
# Diagnostic-only: no command lines, environment dumps, token files, addresses, or payloads.
# Each collector records its own failure separately from the Playwright test result.
$ErrorActionPreference = 'Stop'
$data = [ordered]@{ schemaVersion = 1; utc = [DateTime]::UtcNow.ToString('o'); errors = @() }
function Collect([string]$Name, [scriptblock]$Read) {
  try { $data[$Name] = & $Read }
  catch {
    $data[$Name] = $null
    $data.errors += [ordered]@{ collector = $Name; category = [string]$_.CategoryInfo.Category }
  }
}
Collect 'os' { Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, FreePhysicalMemory, TotalVisibleMemorySize, FreeVirtualMemory, TotalVirtualMemorySize }
Collect 'memoryCounters' { @((Get-Counter '\Memory\Pool Nonpaged Bytes','\Memory\Committed Bytes','\Memory\Commit Limit').CounterSamples | Select-Object Path, CookedValue) }
Collect 'processes' { @(Get-Process | Select-Object Id, ProcessName, Handles, WorkingSet64, PrivateMemorySize64, CPU, @{Name='Threads';Expression={$_.Threads.Count}}) }
Collect 'processTree' { @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CreationDate) }
Collect 'tcp' {
  $connections = @(Get-NetTCPConnection)
  [ordered]@{
    states = @($connections | Group-Object State | Select-Object Name, Count)
    owners = @($connections | Group-Object OwningProcess | Sort-Object Count -Descending | Select-Object -First 20 Name, Count)
  }
}
Collect 'dynamicTcpPorts' {
  $result = @(netsh int ipv4 show dynamicport tcp)
  if ($LASTEXITCODE -ne 0) { throw 'netsh failed' }
  $result
}
Collect 'nodeVersion' { node --version; if ($LASTEXITCODE -ne 0) { throw 'node failed' } }
Collect 'edgeVersion' { (Get-Item 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe').VersionInfo.ProductVersion }
Collect 'imageVersion' { $env:ImageVersion }
$parent = Split-Path -Parent $OutputFile
if ($parent) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
$data | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 $OutputFile
if ($data.errors.Count) {
  Write-Warning 'CI diagnostic collection incomplete; see the errors array. This is not a Playwright failure.'
  exit 1
}
