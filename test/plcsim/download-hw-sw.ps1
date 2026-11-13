# Downloads hardware AND software of PLC_1 to PLCSIM with TIA Portal Openness (the
# TIA Portal MCP DownloadToPlc loads the software only; S7 connections and the IP
# router are hardware data). Answers the download questions like the MCP: stop the
# CPU, overwrite the system data, consistent blocks, start after the download.
#   powershell -ExecutionPolicy Bypass -File download-hw-sw.ps1 -ProjectPath <path>.ap21
param([Parameter(Mandatory = $true)][string]$ProjectPath, [string]$PcInterface = '*PLCSIM*')
$ErrorActionPreference = 'Stop'
$api = 'C:\Program Files\Siemens\Automation\Portal V21\PublicAPI\V21\net48'
foreach ($f in 'Siemens.Engineering.Base', 'Siemens.Engineering.Step7') { [void][Reflection.Assembly]::LoadFrom("$api\$f.dll") }
$proc = [Siemens.Engineering.TiaPortal]::GetProcesses() | Where-Object { $_.ProjectPath -and $_.ProjectPath.FullName -eq $ProjectPath } | Select-Object -First 1
if ($null -eq $proc) { throw "no TIA Portal with the project $ProjectPath open" }
$portal = $proc.Attach()
try {
  $project = $portal.Projects | Where-Object { $_.Path.FullName -eq $ProjectPath } | Select-Object -First 1
  function Find($items, $name) { foreach ($i in $items) { if ($i.Name -eq $name) { return $i }; $r = Find $i.DeviceItems $name; if ($null -ne $r) { return $r } } }
  $cpu = $null; foreach ($d in $project.Devices) { $cpu = Find $d.DeviceItems 'PLC_1'; if ($null -ne $cpu) { break } }
  $dp = $cpu.GetType().GetMethod('GetService').MakeGenericMethod([Siemens.Engineering.Download.DownloadProvider]).Invoke($cpu, $null)
  $pcif = $dp.Configuration.Modes.Find('PN/IE').PcInterfaces | Where-Object { $_.Name -like $PcInterface } | Select-Object -First 1
  $ti = $pcif.TargetInterfaces | Select-Object -First 1
  "route: $($pcif.Name) -> $($ti.Name)"
  $pre = [Siemens.Engineering.Download.DownloadConfigurationDelegate]{
    param($c)
    $choices = @{ 'StopModules' = 'StopAll'; 'ConsistentBlocksDownload' = 'ConsistentDownload'; 'AlarmTextLibrariesDownload' = 'ConsistentDownload'; 'OverwriteSystemData' = 'Overwrite' }
    $n = $c.GetType().Name
    if ($choices.ContainsKey($n)) { $c.CurrentSelection = $choices[$n] }
    Write-Host "  pre: $n"
  }
  $post = [Siemens.Engineering.Download.DownloadConfigurationDelegate]{
    param($c)
    if ($c.GetType().Name -eq 'StartModules') { $c.CurrentSelection = 'StartModule' }
    Write-Host "  post: $($c.GetType().Name)"
  }
  $r = $dp.Download($ti, $pre, $post, [Siemens.Engineering.Download.DownloadOptions]'Hardware, Software')
  "download: $($r.State) errors=$($r.ErrorCount) warnings=$($r.WarningCount)"
} finally {
  $portal.Dispose()
}
