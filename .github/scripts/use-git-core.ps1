# Only for the x64 Git for Windows installation on GitHub-hosted Windows test runners.
$ErrorActionPreference = 'Stop'
$root = 'C:\Program Files\Git'
$coreBin = "$root\mingw64\bin"
$core = "$coreBin\git.exe"
$wrapper = "$root\bin\git.exe"
foreach ($file in @($core, $wrapper, "$root\usr\bin\sh.exe")) {
    if (!(Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Unsupported runner Git layout: missing $file. Core Git was not enabled."
    }
}

# Compare only the wrapper's relevant environment and selected non-secret config/tool paths.
# The shell alias also exercises Git's ability to launch its bundled shell and find helpers.
$check = @'
alias.ci-check=!printf '%s\n' "$HOME" "$MSYSTEM" "$PLINK_PROTOCOL" "$MSYS" && git --version && git --exec-path && git config --show-origin --get-regexp '^(core.autocrlf|core.symlinks|core.longpaths|init.defaultbranch)$' && command -v sh && command -v git-upload-pack && command -v ssh
'@
function Read-GitProfile([string] $Git) {
    $result = & $Git -c $check ci-check
    if ($LASTEXITCODE -ne 0) { throw "Git compatibility check failed for $Git" }
    return $result -join "`n"
}
$before = Read-GitProfile $wrapper

# The core initializes HOME, PLINK_PROTOCOL, MSYSTEM and Git's helper PATH itself.
# The wrapper additionally reads MSYS from git-bash.config (e.g. native symlink support).
# Apply just that setting once, preserving any existing MSYS options and the entire PATH.
$msysConfig = "$root\etc\git-bash.config"
if (Test-Path -LiteralPath $msysConfig) {
    foreach ($line in Get-Content -LiteralPath $msysConfig) {
        if ($line.StartsWith('MSYS=')) {
            $env:MSYS = ($line.Substring(5) + ' ' + $env:MSYS).Trim()
        }
    }
}
$env:PATH = "$coreBin;$env:PATH"
if ((Read-GitProfile $core) -cne $before) {
    throw 'Core Git differs from the wrapper in environment, config or helper resolution. Not enabling it.'
}
$execPath = & $core --exec-path
if ($LASTEXITCODE -ne 0 -or $execPath.Replace('/', '\') -ine "$root\mingw64\libexec\git-core") {
    throw "Unsupported Git runtime layout: $execPath"
}

# Inspect a real process image, not just PowerShell's command lookup. cat-file waits for input;
# read one response first so the process is running, then inspect it while it awaits another line.
$start = [System.Diagnostics.ProcessStartInfo]::new('git', 'cat-file --batch-check')
$start.UseShellExecute = $false
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$process = [System.Diagnostics.Process]::Start($start)
try {
    $process.StandardInput.WriteLine('HEAD')
    $process.StandardInput.Flush()
    $object = $process.StandardOutput.ReadLine()
    if ($object -notmatch '^[0-9a-f]{40,64} commit [0-9]+$') { throw 'Git could not read the checked-out commit.' }
    $image = $process.MainModule.FileName
    if ($image -ine $core) { throw "Git started $image instead of $core" }
    $process.StandardInput.Close()
    if (!$process.WaitForExit(10000)) { throw 'Git did not exit after stdin closed.' }
    if ($process.ExitCode -ne 0) { throw "Git exited with $($process.ExitCode)" }
} finally {
    if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
    $process.Dispose()
}

# Publish only after all checks pass. Existing Git cmd/bin, SSH and other tool paths remain.
$coreBin | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
if ($env:MSYS) { "MSYS=$env:MSYS" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append }
Write-Host "Git process image: $image"
& $core --version
Write-Host "Git exec path: $execPath"
Write-Host 'Wrapper/core environment, selected config and shell/helper checks passed.'
