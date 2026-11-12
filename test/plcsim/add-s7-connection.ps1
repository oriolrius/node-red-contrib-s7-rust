# Adds the S7 connection for the BSEND/BRCV tests (FB PartnerComm) to PLC_1 with TIA
# Portal Openness: an S7 connection to an unspecified partner, passive on the PLC
# side, numeric TSAPs (PLC 10.01 with resource 16#10, partner 03.00), ID 16#100.
# Also turns on the router of the PLC interface when the partner is in another subnet.
# Run in Windows PowerShell 5.1 with the project open in TIA Portal V21 (accept the
# Openness access dialog), then download hardware AND software (download-hw-sw.ps1):
# the MCP DownloadToPlc loads the software only, and the connection is hardware data.
#   powershell -ExecutionPolicy Bypass -File add-s7-connection.ps1 -ProjectPath <path>.ap21 -PartnerIp 192.168.0.10
param(
  [Parameter(Mandatory = $true)][string]$ProjectPath,
  # the address the PLC sees for Node-RED (PLCSIM on the same PC: the PC address on the PLCSIM network)
  [Parameter(Mandatory = $true)][string]$PartnerIp,
  [string]$Router = ''
)
$ErrorActionPreference = 'Stop'
$api = 'C:\Program Files\Siemens\Automation\Portal V21\PublicAPI\V21\net48'
# load both assemblies first (an AssemblyResolve script block recurses in PowerShell)
foreach ($f in 'Siemens.Engineering.Base', 'Siemens.Engineering.Step7') { [void][Reflection.Assembly]::LoadFrom("$api\$f.dll") }
$proc = [Siemens.Engineering.TiaPortal]::GetProcesses() | Where-Object { $_.ProjectPath -and $_.ProjectPath.FullName -eq $ProjectPath } | Select-Object -First 1
if ($null -eq $proc) { throw "no TIA Portal with the project $ProjectPath open" }
$portal = $proc.Attach()
try {
  $project = $portal.Projects | Where-Object { $_.Path.FullName -eq $ProjectPath } | Select-Object -First 1
  function Find($items, $name) { foreach ($i in $items) { if ($i.Name -eq $name) { return $i }; $r = Find $i.DeviceItems $name; if ($null -ne $r) { return $r } } }
  $cpu = $null; $pn = $null
  foreach ($d in $project.Devices) { $cpu = Find $d.DeviceItems 'PLC_1'; $pn = Find $d.DeviceItems 'PROFINET interface_1'; if ($null -ne $cpu) { break } }
  if ($null -eq $cpu -or $null -eq $pn) { throw 'PLC_1 / PROFINET interface_1 not found' }
  $ni = $pn.GetType().GetMethod('GetService').MakeGenericMethod([Siemens.Engineering.HW.Features.NetworkInterface]).Invoke($pn, $null)
  $node = $ni.Nodes[0]
  if ($Router) { $node.SetAttribute('UseRouter', $true); $node.SetAttribute('RouterAddress', $Router); "router $Router" }
  $cm = $cpu.GetType().GetMethod('GetService').MakeGenericMethod([Siemens.Engineering.HW.Features.CommunicationManagement]).Invoke($cpu, $null)
  $c = $cm.Connections | Where-Object { $_.ConnectionType -eq 'S7Connection' } | Select-Object -First 1
  if ($null -eq $c) {
    $m = $cm.Connections.GetType().GetMethod('Create').MakeGenericMethod([Siemens.Engineering.HW.CommunicationConnections.S7Connection])
    $c = $m.Invoke($cm.Connections, @($node, $null, $null))
    'created S7 connection'
  }
  $c.SetAttribute('LocalUseSimaticACC', $false)
  $c.SetAttribute('LocalActiveEstablishment', $false)
  $c.SetAttribute('LocalConnectionResourceId', [long]16)
  $c.SetAttribute('PartnerAddress', $PartnerIp)
  foreach ($p in 'LocalConnectionName', 'LocalConnectionId', 'LocalTsap', 'PartnerAddress', 'PartnerTsap', 'IsValid') { "  $p = $($c.$p)" }
} finally {
  $portal.Dispose()
}
