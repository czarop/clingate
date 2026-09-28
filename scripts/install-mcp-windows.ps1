<#
Build clingate's tools for Claude and add them to Claude Desktop, on Windows.

Checks for Git, Rust and the Visual Studio C++ build tools, and offers to
install whichever is missing with winget. Then: checks git can reach the
private repositories (Git Credential Manager signs you in to GitHub in the
browser the first time), clones or updates clingate, builds the server,
copies it to %USERPROFILE%\bin, adds it to Claude Desktop's config (backing
the old one up), and checks the program answers as Claude Desktop will ask it.

Run it in PowerShell - from anywhere - with:

    powershell -ExecutionPolicy Bypass -File .\install-mcp-windows.ps1

Run it again to update. Settings, if the defaults do not suit:

    -Dir     where the code is kept   (default %USERPROFILE%\clingate)
    -Branch  the branch to build      (default below)
    -Bin     where the program goes   (default %USERPROFILE%\bin)
#>

param(
    # The server is on this branch until it is merged; then this becomes `main`.
    [string]$Branch = "claude/funny-bardeen-bleqcn",
    [string]$Dir = (Join-Path $env:USERPROFILE "clingate"),
    [string]$Bin = (Join-Path $env:USERPROFILE "bin")
)

$ErrorActionPreference = "Stop"
$Repo = "https://github.com/czarop/clingate"
$Flow = "https://github.com/czarop/flow"

function Step($text) { Write-Host "`n==> $text" -ForegroundColor Cyan }
function Fail($text) { Write-Host "`nStopped: $text" -ForegroundColor Red; exit 1 }
function Ask($question) { (Read-Host "$question [y/N]") -match '^[Yy]' }

# Run a program - git, cargo - and say whether it succeeded. Under "Stop",
# Windows PowerShell 5 treats anything a program writes to stderr as an error
# once it is redirected, and git writes its progress there; so programs run
# under "Continue" and are judged by their exit code alone.
function Try-Native {
    param([string]$Exe, [string[]]$Arguments, [switch]$Quiet)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        if ($Quiet) { & $Exe @Arguments *> $null } else { & $Exe @Arguments | Out-Host }
        return ($LASTEXITCODE -eq 0)
    } finally {
        $ErrorActionPreference = $previous
    }
}

# The arguments as an array, never bare: PowerShell would take `-C` or `-p`
# as a parameter of this function rather than pass it to the program.
function Run([string]$Exe, [string[]]$Arguments) {
    if (-not (Try-Native $Exe $Arguments)) { Fail "'$Exe $($Arguments -join ' ')' failed" }
}

# Programs installed a moment ago are on the machine's PATH, not yet on this
# window's.
function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" +
                [Environment]::GetEnvironmentVariable("Path", "User")
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if ((Test-Path $cargoBin) -and ($env:Path -notlike "*$cargoBin*")) { $env:Path += ";$cargoBin" }
}

function Has($name) { [bool](Get-Command $name -ErrorAction SilentlyContinue) }

function Install-With-Winget($what, $arguments) {
    if (-not (Has "winget")) {
        Fail "$what is missing and winget is not available to install it. Install it by hand, then run this again."
    }
    if (-not (Ask "$what is missing. Install it with winget now?")) {
        Fail "$what is needed. Install it, then run this again."
    }
    if (-not (Try-Native "winget" $arguments)) { Fail "winget could not install $what" }
    Refresh-Path
}

# ── what has to be here ──────────────────────────────────────────────────
Step "Checking the tools this needs"
Refresh-Path

if (-not (Has "git")) {
    Install-With-Winget "Git" @("install", "--id", "Git.Git", "-e", "--source", "winget",
        "--accept-package-agreements", "--accept-source-agreements")
}

# The C++ compiler and the Windows SDK, which Rust's msvc toolchain links with.
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$hasCpp = (Test-Path $vswhere) -and
    [bool](& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if (-not $hasCpp) {
    Write-Host "This takes a while and may ask for administrator permission."
    Install-With-Winget "Visual Studio C++ build tools" @("install", "--id", "Microsoft.VisualStudio.2022.BuildTools", "-e",
        "--source", "winget", "--accept-package-agreements", "--accept-source-agreements",
        "--override", "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended")
}

if (-not (Has "cargo")) {
    Install-With-Winget "Rust" @("install", "--id", "Rustlang.Rustup", "-e", "--source", "winget",
        "--accept-package-agreements", "--accept-source-agreements")
    if (-not (Has "cargo")) { Fail "Rust was installed but cargo is not found. Open a new PowerShell window and run this again." }
}
Write-Host "$(git --version), $(cargo --version)"

# ── GitHub: both repositories are private ────────────────────────────────
Step "Checking git can reach the private repositories"
Write-Host "If a GitHub sign-in window opens, sign in with the account that can see czarop/clingate."
if (-not (Try-Native "git" @("ls-remote", $Flow) -Quiet)) {
    Fail "git cannot reach $Flow. Check your GitHub account can see czarop/flow."
}
if (-not (Try-Native "git" @("ls-remote", $Repo) -Quiet)) {
    Fail "git cannot reach $Repo. Check your GitHub account can see czarop/clingate."
}
Write-Host "Both repositories are reachable."

# ── the code ─────────────────────────────────────────────────────────────
if (Test-Path (Join-Path $Dir ".git")) {
    Step "Updating $Dir to $Branch"
    $changes = & git -C $Dir status --porcelain --untracked-files=no
    if ($changes) { Fail "$Dir has changes of your own. Commit or stash them, then run this again." }
    Run "git" @("-C", $Dir, "fetch", "origin", $Branch)
    if (-not (Try-Native "git" @("-C", $Dir, "checkout", $Branch) -Quiet)) {
        Run "git" @("-C", $Dir, "checkout", "-b", $Branch, "origin/$Branch")
    }
    Run "git" @("-C", $Dir, "merge", "--ff-only", "origin/$Branch")
} else {
    if (Test-Path $Dir) { Fail "$Dir exists but is not a clone of clingate. Move it, or pass -Dir." }
    Step "Cloning clingate ($Branch) into $Dir"
    Run "git" @("clone", "--branch", $Branch, $Repo, $Dir)
}

# ── build ────────────────────────────────────────────────────────────────
Step "Building the server (the first build takes several minutes)"
Push-Location $Dir
try { Run "cargo" @("build", "--release", "-p", "clingate-mcp") } finally { Pop-Location }

# Windows will not replace a program that is running: Claude Desktop runs it.
$claude = Get-Process -Name "Claude" -ErrorAction SilentlyContinue
$server = Get-Process -Name "clingate-mcp" -ErrorAction SilentlyContinue
$restart = $false
if ($claude -or $server) {
    Step "Claude Desktop is running"
    if (-not (Ask "It has to be closed to replace the program. Close it now (and reopen it at the end)?")) {
        Fail "Quit Claude Desktop from the system tray, then run this again - the build is done, so it will be quick."
    }
    $claude | Stop-Process -Force -ErrorAction SilentlyContinue
    $server | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    $restart = $true
}

Step "Installing it in $Bin"
New-Item -ItemType Directory -Force -Path $Bin | Out-Null
$Program = Join-Path $Bin "clingate-mcp.exe"
Copy-Item (Join-Path $Dir "target\release\clingate-mcp.exe") $Program -Force -ErrorAction Stop
Write-Host $Program

# ── check it answers ─────────────────────────────────────────────────────
Step "Checking the server answers as Claude Desktop will ask it"
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = $Program
$start.UseShellExecute = $false
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $false
$start.CreateNoWindow = $true
$check = [System.Diagnostics.Process]::Start($start)
try {
    $check.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"install-check","version":"0"}}}')
    $check.StandardInput.Flush()
    $reading = $check.StandardOutput.ReadLineAsync()
    if (-not $reading.Wait(30000)) { Fail "the server did not answer within 30 seconds" }
    $answer = $reading.Result | ConvertFrom-Json
    if ($answer.result.serverInfo.name -ne "clingate") { Fail "the server answered unexpectedly: $($reading.Result)" }
    $check.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized"}')
    $check.StandardInput.WriteLine('{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}')
    $check.StandardInput.Flush()
    $reading = $check.StandardOutput.ReadLineAsync()
    if (-not $reading.Wait(30000)) { Fail "the server did not list its tools within 30 seconds" }
    $tools = ($reading.Result | ConvertFrom-Json).result.tools
    Write-Host "It answers, with $($tools.Count) tools."
} finally {
    if (-not $check.HasExited) { $check.Kill() }
}

# ── Claude Desktop's config ──────────────────────────────────────────────
Step "Adding it to Claude Desktop"
# The installer from claude.ai keeps its config in %APPDATA%\Claude. A copy
# from the Microsoft Store keeps it inside its package folder instead. Every
# one that is here is updated.
$configs = @(Join-Path $env:APPDATA "Claude\claude_desktop_config.json")
Get-ChildItem (Join-Path $env:LOCALAPPDATA "Packages") -Directory -Filter "Claude_*" -ErrorAction SilentlyContinue |
    ForEach-Object {
        $store = Join-Path $_.FullName "LocalCache\Roaming\Claude"
        if (Test-Path $store) { $configs += (Join-Path $store "claude_desktop_config.json") }
    }

foreach ($config in $configs) {
    New-Item -ItemType Directory -Force -Path (Split-Path $config) | Out-Null
    $settings = New-Object PSObject
    if ((Test-Path $config) -and ((Get-Item $config).Length -gt 0)) {
        $backup = "$config.backup-$(Get-Date -Format yyyyMMdd-HHmmss)"
        Copy-Item $config $backup
        Write-Host "The old config is kept as $backup"
        try {
            $settings = Get-Content $config -Raw | ConvertFrom-Json
        } catch {
            Fail "$config is not valid JSON; fix it by hand, then run this again"
        }
    }
    if (-not $settings.PSObject.Properties["mcpServers"]) {
        $settings | Add-Member -NotePropertyName mcpServers -NotePropertyValue (New-Object PSObject)
    }
    $entry = $settings.mcpServers.PSObject.Properties["clingate"]
    if ($entry) {
        # Anything else there - an env block - is kept.
        $entry.Value | Add-Member -NotePropertyName command -NotePropertyValue $Program -Force
    } else {
        $settings.mcpServers | Add-Member -NotePropertyName clingate -NotePropertyValue ([PSCustomObject]@{ command = $Program })
    }
    $json = $settings | ConvertTo-Json -Depth 32
    # UTF-8 without a byte-order mark: with one, the file may not be read.
    [System.IO.File]::WriteAllText($config, $json, (New-Object System.Text.UTF8Encoding $false))
    Write-Host "${config}: clingate -> $Program"
}

# ── restart ──────────────────────────────────────────────────────────────
Step "Done"
$claudeExe = @(
    (Join-Path $env:LOCALAPPDATA "AnthropicClaude\claude.exe"),
    (Join-Path $env:LOCALAPPDATA "Programs\Claude\Claude.exe")
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($restart -and $claudeExe) {
    Start-Process $claudeExe
    Write-Host "Claude Desktop is starting again."
} elseif ($restart) {
    Write-Host "Open Claude Desktop again from the Start menu."
} else {
    Write-Host "Open Claude Desktop: the clingate tools are under the tools menu in a new chat."
}
Write-Host @"

If the tools do not appear, Claude Desktop's log says why:
    $env:APPDATA\Claude\logs\mcp-server-clingate.log
In Claude Desktop, Settings > Developer > Edit Config shows the config it reads.
"@
