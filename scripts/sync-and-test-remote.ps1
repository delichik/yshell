[CmdletBinding()]
param(
    [string]$RemoteHost = "",
    [string]$RemoteUser = "root",
    [string]$RemoteDir = "/root/yshell",
    [string]$RemoteArchive = "/tmp/yshell-sync.tgz",
    [string]$TestCommand = "cargo xtask test",
    [string[]]$Path,
    [switch]$FullSync,
    [switch]$SkipSync,
    [switch]$SkipTest,
    [switch]$KeepArchive
)


if ([string]::IsNullOrWhiteSpace($RemoteHost)) {
    throw "RemoteHost is required. The old Debian validation VM has been retired; pass -RemoteHost explicitly or use the build tool and the container workflow instead (cargo xtask ..., scripts/test-ssh/run.sh)."
}

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

$sshExe = "C:\Windows\System32\OpenSSH\ssh.exe"
$scpExe = "C:\Windows\System32\OpenSSH\scp.exe"

function Assert-ToolExists {
    param(
        [string]$LiteralPath,
        [string]$DisplayName
    )

    if (-not (Test-Path -LiteralPath $LiteralPath)) {
        throw "$DisplayName not found at $LiteralPath"
    }
}

function Quote-RemoteArg {
    param([string]$Value)
    return '"' + ($Value -replace '(["\\$`])', '\\$1') + '"'
}

function Get-StatusPaths {
    $statusLines = & git -C $repoRoot status --porcelain=v1 --untracked-files=all
    $uploadSet = [System.Collections.Generic.HashSet[string]]::new()
    $deleteSet = [System.Collections.Generic.HashSet[string]]::new()

    foreach ($line in $statusLines) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }

        $status = $line.Substring(0, 2)
        $payload = $line.Substring(3)

        if ($status.Contains("R")) {
            $parts = $payload -split " -> ", 2
            if ($parts.Count -eq 2) {
                $deleteSet.Add($parts[0]) | Out-Null
                $uploadSet.Add($parts[1]) | Out-Null
            }
            continue
        }

        if ($status.Contains("D")) {
            $deleteSet.Add($payload) | Out-Null
            continue
        }

        $uploadSet.Add($payload) | Out-Null
    }

    [pscustomobject]@{
        Upload = @($uploadSet)
        Delete = @($deleteSet)
    }
}

function Get-FullSyncPaths {
    $tracked = & git -C $repoRoot ls-files
    $untracked = & git -C $repoRoot ls-files --others --exclude-standard
    $uploadSet = [System.Collections.Generic.HashSet[string]]::new()

    foreach ($pathItem in @($tracked) + @($untracked)) {
        if (-not [string]::IsNullOrWhiteSpace($pathItem)) {
            $uploadSet.Add($pathItem) | Out-Null
        }
    }

    [pscustomobject]@{
        Upload = @($uploadSet)
        Delete = @()
    }
}

function Get-ExplicitPaths {
    param([string[]]$RequestedPaths)

    $uploadSet = [System.Collections.Generic.HashSet[string]]::new()
    foreach ($requestedPath in $RequestedPaths) {
        if ([string]::IsNullOrWhiteSpace($requestedPath)) {
            continue
        }

        $fullPath = Join-Path $repoRoot $requestedPath
        if (Test-Path -LiteralPath $fullPath) {
            $uploadSet.Add($requestedPath) | Out-Null
        }
        else {
            throw "Requested path does not exist: $requestedPath"
        }
    }

    [pscustomobject]@{
        Upload = @($uploadSet)
        Delete = @()
    }
}

function Invoke-RemoteCommand {
    param([string]$CommandText)

    & $sshExe "$RemoteUser@$RemoteHost" $CommandText
    if ($LASTEXITCODE -ne 0) {
        throw "Remote command failed with exit code $LASTEXITCODE"
    }
}

function Sync-Workspace {
    param(
        [string[]]$UploadPaths,
        [string[]]$DeletePaths
    )

    if ($UploadPaths.Count -eq 0 -and $DeletePaths.Count -eq 0) {
        Write-Host "No workspace changes to sync."
        return
    }

    $tempBase = Join-Path $env:TEMP ("yshell-remote-sync-" + [guid]::NewGuid().ToString("N"))
    $archivePath = "$tempBase.tgz"
    $fileListPath = "$tempBase-files.txt"

    try {
        if ($UploadPaths.Count -gt 0) {
            $normalizedPaths = $UploadPaths | Sort-Object
            Set-Content -Path $fileListPath -Value $normalizedPaths -Encoding ascii
            & tar.exe -czf $archivePath -C $repoRoot -T $fileListPath
            if ($LASTEXITCODE -ne 0) {
                throw "Failed to build sync archive."
            }

            & $scpExe $archivePath "$RemoteUser@$RemoteHost`:$RemoteArchive"
            if ($LASTEXITCODE -ne 0) {
                throw "Failed to upload sync archive."
            }

            $remoteExtract = "mkdir -p $(Quote-RemoteArg $RemoteDir) && tar -xzf $(Quote-RemoteArg $RemoteArchive) -C $(Quote-RemoteArg $RemoteDir)"
            Invoke-RemoteCommand -CommandText $remoteExtract
        }

        if ($DeletePaths.Count -gt 0) {
            $deleteArgs = $DeletePaths |
                Sort-Object |
                ForEach-Object { Quote-RemoteArg (($_ -replace "\\", "/")) }

            $remoteDelete = "cd $(Quote-RemoteArg $RemoteDir) && rm -f -- $($deleteArgs -join ' ')"
            Invoke-RemoteCommand -CommandText $remoteDelete
        }

        if (-not $KeepArchive) {
            Invoke-RemoteCommand -CommandText "rm -f -- $(Quote-RemoteArg $RemoteArchive)"
        }
    }
    finally {
        Remove-Item -LiteralPath $archivePath -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $fileListPath -Force -ErrorAction SilentlyContinue
    }
}

Assert-ToolExists -LiteralPath $sshExe -DisplayName "ssh"
Assert-ToolExists -LiteralPath $scpExe -DisplayName "scp"

$syncPlan =
    if ($null -ne $Path -and $Path.Count -gt 0) {
        Get-ExplicitPaths -RequestedPaths $Path
    }
    elseif ($FullSync) {
        Get-FullSyncPaths
    }
    else {
        Get-StatusPaths
    }

Write-Host "Remote host : $RemoteUser@$RemoteHost"
Write-Host "Remote dir  : $RemoteDir"
Write-Host "Upload files: $($syncPlan.Upload.Count)"
Write-Host "Delete files: $($syncPlan.Delete.Count)"

if (-not $SkipSync) {
    Sync-Workspace -UploadPaths $syncPlan.Upload -DeletePaths $syncPlan.Delete
}

if (-not $SkipTest) {
    $remoteTest = "cd $(Quote-RemoteArg $RemoteDir) && $TestCommand"
    Invoke-RemoteCommand -CommandText $remoteTest
}
