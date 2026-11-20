$sdirWindowsReleaseTarget = 'x86_64-pc-windows-msvc'

function Invoke-SdirWindowsReleaseBuild {
    param([Parameter(Mandatory=$true)][string]$Cargo)

    $flagName = 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS'
    $separator = ' '
    if ($null -ne $env:CARGO_ENCODED_RUSTFLAGS) {
        $flagName = 'CARGO_ENCODED_RUSTFLAGS'
        $separator = [string][char]31
    } elseif ($null -ne $env:RUSTFLAGS) {
        $flagName = 'RUSTFLAGS'
    }

    $previous = [Environment]::GetEnvironmentVariable($flagName, 'Process')
    $flags = '-C' + $separator + 'target-feature=+crt-static'
    # A configured build-level environment value is the fallback when no higher-priority flags are set.
    $inherited = $previous
    if ($flagName -eq 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS' -and $null -eq $previous) {
        $inherited = $env:CARGO_BUILD_RUSTFLAGS
    }
    if ($inherited) { $flags = $inherited + $separator + $flags }
    [Environment]::SetEnvironmentVariable($flagName, $flags, 'Process')
    try {
        & $Cargo build --locked --release --features desktop --target $sdirWindowsReleaseTarget
    } finally {
        [Environment]::SetEnvironmentVariable($flagName, $previous, 'Process')
    }
}
