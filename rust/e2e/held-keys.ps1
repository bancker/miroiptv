# Shared by run-scenario.ps1 and measure-startup.ps1: refuse to run while a key
# the scenarios depend on is held down.
#
# winit turns a key that Windows reports as down into a key press when its window
# gets the focus. A stuck Tab (or a held Shift, Ctrl, Alt, Enter, Esc, Space or
# arrow key) therefore changes what a scenario does, and a result then depends on
# the state of the keyboard instead of on the app. This only reads the keyboard
# state (GetAsyncKeyState); it sends nothing.
Add-Type -Namespace E2E -Name Keys -MemberDefinition @'
[DllImport("user32.dll")] public static extern short GetAsyncKeyState(int vKey);
'@

# The keys that matter and their virtual-key codes.
$script:WatchedKeys = [ordered]@{
    'Tab' = 0x09; 'Shift' = 0x10; 'Ctrl' = 0x11; 'Alt' = 0x12; 'Enter' = 0x0D
    'Esc' = 0x1B; 'Space' = 0x20
    'Left arrow' = 0x25; 'Up arrow' = 0x26; 'Right arrow' = 0x27; 'Down arrow' = 0x28
}

# The names of the watched keys that are down now. -IsDown takes a virtual-key code
# and says whether that key is down (the tests of this file give it a fake).
function Get-HeldKeys([scriptblock]$IsDown = { param($vk) ([E2E.Keys]::GetAsyncKeyState($vk) -band 0x8000) -ne 0 }) {
    @($script:WatchedKeys.Keys | Where-Object { & $IsDown $script:WatchedKeys[$_] })
}

# Prints which keys are held and ends the calling script with exit code 1; returns
# quietly when none is.
function Assert-KeysUp([scriptblock]$IsDown) {
    $held = if ($IsDown) { Get-HeldKeys $IsDown } else { Get-HeldKeys }
    if ($held.Count) {
        Write-Host ("e2e: refusing to start - {0} {1} held down (GetAsyncKeyState)." -f ($held -join ', '), $(if ($held.Count -eq 1) { 'is' } else { 'are' }))
        Write-Host 'winit turns a key Windows reports as down into a key press when the window gets the focus,'
        Write-Host 'so a stuck key changes what the scenario does. Press and release it once, then run again.'
        exit 1
    }
}
