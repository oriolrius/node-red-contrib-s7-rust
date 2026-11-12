# Sets the alarm class of the AlarmGen alarms (alReq: "Acknowledgement") with TIA
# Portal Openness: imports alarm-classes.xlsx (export of the instance alarm texts,
# column "Alarm class"). The TIA Portal Openness MCP has no import of alarm texts.
# Run in Windows PowerShell 5.1 with the project open in TIA Portal V21; accept the
# Openness access dialog of TIA Portal. Then compile and download.
#   powershell -ExecutionPolicy Bypass -File set-alarm-class.ps1 -ProjectPath <path>.ap21
param([Parameter(Mandatory = $true)][string]$ProjectPath, [string]$Xlsx = "")
$ErrorActionPreference = 'Stop'
# $PSScriptRoot is empty in a param default of Windows PowerShell 5.1
if (-not $Xlsx) { $Xlsx = Join-Path $PSScriptRoot 'alarm-classes.xlsx' }
$api = 'C:\Program Files\Siemens\Automation\Portal V21\PublicAPI\V21\net48'
# load both assemblies first (an AssemblyResolve script block recurses in PowerShell)
foreach ($f in 'Siemens.Engineering.Base', 'Siemens.Engineering.Step7') { [void][Reflection.Assembly]::LoadFrom("$api\$f.dll") }
$proc = [Siemens.Engineering.TiaPortal]::GetProcesses() | Where-Object { $_.ProjectPath -and $_.ProjectPath.FullName -eq $ProjectPath } | Select-Object -First 1
if ($null -eq $proc) { throw "no TIA Portal with the project $ProjectPath open" }
$portal = $proc.Attach()
try {
  $project = $portal.Projects | Where-Object { $_.Path.FullName -eq $ProjectPath } | Select-Object -First 1
  $sw = $null
  foreach ($d in $project.Devices) {
    foreach ($di in $d.DeviceItems) {
      $gs = $di.GetType().GetMethod('GetService').MakeGenericMethod([Siemens.Engineering.HW.Features.SoftwareContainer])
      $sc = $gs.Invoke($di, $null)
      if ($null -ne $sc) { $sw = $sc.Software }
    }
  }
  if ($null -eq $sw) { throw 'no PLC software in the project' }
  $gp = $sw.GetType().GetMethod('GetService').MakeGenericMethod([Siemens.Engineering.SW.Alarm.PlcAlarmTextProvider])
  $prov = $gp.Invoke($sw, $null)
  $langs = @($project.LanguageSettings.Languages | Where-Object { $_.Culture.Name -eq 'en-US' })
  $r = $prov.ImportInstanceTextsFromXlsx([IO.FileInfo]$Xlsx, [Siemens.Engineering.Language[]]$langs)
  "import: $($r.State) (log: $($r.LogFilePath))"
} finally {
  $portal.Dispose()
}
