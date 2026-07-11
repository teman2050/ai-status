# Allow ESP32 devices on the LAN to reach the aistatusplus bridge:
#   TCP $Port           - the device API itself
#   UDP $DiscoveryPort  - pairing broadcasts (devices find the PC's IP automatically)
# Run from an elevated (Administrator) PowerShell:
#   powershell -ExecutionPolicy Bypass -File scripts\add-firewall-rule.ps1
param(
    [int]$Port = 7788,
    [int]$DiscoveryPort = 7789
)

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Please run this script from an elevated (Administrator) PowerShell."
    exit 1
}

$rules = @(
    @{ Name = "aistatusplus bridge (TCP $Port)"; Protocol = "TCP"; LocalPort = $Port },
    @{ Name = "aistatusplus discovery (UDP $DiscoveryPort)"; Protocol = "UDP"; LocalPort = $DiscoveryPort }
)

foreach ($r in $rules) {
    if (Get-NetFirewallRule -DisplayName $r.Name -ErrorAction SilentlyContinue) {
        Write-Host "Already exists: $($r.Name)"
        continue
    }
    New-NetFirewallRule -DisplayName $r.Name `
        -Direction Inbound -Protocol $r.Protocol -LocalPort $r.LocalPort `
        -Action Allow -Profile Private | Out-Null
    Write-Host "Added inbound rule: $($r.Name) (Private network profile only)"
}

Write-Host "If your Wi-Fi is classified as a Public network, reclassify it as Private"
Write-Host "in Windows settings, or adjust the rules' -Profile manually."
