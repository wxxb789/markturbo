param(
    [Parameter(Mandatory = $true)]
    [string] $InstallDir
)

# Uninstall owns no documents, settings or recovery data. This helper only
# closes MarkTurbo processes from this installation, never an image-name tree.
# An unreadable live executable path is never closed or killed: fail closed so
# the uninstaller aborts instead. Starting an instance after the final scan is
# still a launch race; do not start MarkTurbo while uninstall is in progress.
$ErrorActionPreference = 'Stop'
$installRoot = [System.IO.Path]::GetFullPath($InstallDir).TrimEnd('\') + '\'

function Get-InstalledProcesses {
    $matches = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
    try {
        foreach ($candidate in [System.Diagnostics.Process]::GetProcessesByName('MarkTurbo')) {
            $selected = $false
            try {
                # Hold the process handle before inspecting its path. Keeping it
                # open prevents PID reuse during the close/wait/kill sequence.
                $null = $candidate.Handle
                if ($candidate.HasExited) {
                    continue
                }
                $exe = [System.IO.Path]::GetFullPath($candidate.MainModule.FileName)
                if ($exe.StartsWith($installRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
                    $matches.Add($candidate)
                    $selected = $true
                }
            } catch {
                if (-not $candidate.HasExited) {
                    # An unreadable live process cannot safely be scoped.
                    throw
                }
            } finally {
                if (-not $selected) {
                    $candidate.Dispose()
                }
            }
        }
        return $matches.ToArray()
    } catch {
        foreach ($match in $matches) {
            $match.Dispose()
        }
        throw
    }
}

$targets = @()
try {
    $targets = @(Get-InstalledProcesses)
    foreach ($target in $targets) {
        try {
            if (-not $target.HasExited) {
                # WM_CLOSE follows the normal application Save/Discard guard.
                $null = $target.CloseMainWindow()
            }
        } catch {
            if (-not $target.HasExited) {
                throw
            }
        }
    }

    # All instances share one 30-second grace period, not 30 seconds apiece.
    $wait = [System.Diagnostics.Stopwatch]::StartNew()
    foreach ($target in $targets) {
        $remaining = [Math]::Max(0, 30000 - [int] $wait.ElapsedMilliseconds)
        $null = $target.WaitForExit($remaining)
    }

    foreach ($target in $targets) {
        if (-not $target.HasExited) {
            $target.Refresh()
            if ($target.MainWindowHandle -ne [IntPtr]::Zero -and $target.Responding) {
                # A save prompt or cancelled close is not an unresponsive app.
                throw 'MarkTurbo is still responding. Finish its Save/Discard prompt or close its window before uninstalling.'
            }
        }
    }

    foreach ($target in $targets) {
        try {
            if (-not $target.HasExited) {
                # Kill this pinned, path-checked survivor only, never /IM or /T.
                $target.Kill()
            }
        } catch {
            if (-not $target.HasExited) {
                throw
            }
        }
    }

    $wait.Restart()
    foreach ($target in $targets) {
        $remaining = [Math]::Max(0, 5000 - [int] $wait.ElapsedMilliseconds)
        if (-not $target.WaitForExit($remaining)) {
            throw 'A MarkTurbo process did not terminate after the forced-close wait.'
        }
    }

    # Do not silently ignore an instance started while the helper was waiting.
    $remainingTargets = @(Get-InstalledProcesses)
    try {
        if ($remainingTargets.Count -ne 0) {
            throw 'Another MarkTurbo instance started during uninstall. Close it before uninstalling.'
        }
    } finally {
        foreach ($target in $remainingTargets) {
            $target.Dispose()
        }
    }
    exit 0
} catch {
    Write-Error $_ -ErrorAction Continue
    exit 1
} finally {
    foreach ($target in $targets) {
        $target.Dispose()
    }
}
