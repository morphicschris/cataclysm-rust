<#
.SYNOPSIS
    Builds Homeworld: Cataclysm (Rust) and imports the game files from your own installs.

.DESCRIPTION
    1. Checks for the Rust toolchain (offers to install it with winget if missing).
    2. Asks for your Homeworld: Cataclysm install folder (required) and, optionally, your
       Homeworld Remastered folder (only used for the optional HW2-style explosions, engine
       trails and extra battle chatter). Both are only read, never changed.
    3. Builds the game and the importer in release mode.
    4. Copies them to the install folder (default: .\game) and imports the data into
       <install folder>\data.
    5. Writes "Play Cataclysm.cmd" there and a desktop shortcut.

    Run with no arguments for the interactive version (folder pickers). Pass -NonInteractive
    and the folders as arguments for an unattended install (for scripts and AI agents).

.EXAMPLE
    .\install.ps1
.EXAMPLE
    .\install.ps1 -NonInteractive -CataclysmDir "D:\Games\Homeworld Cataclysm\Cataclysm" -NoRemastered
.EXAMPLE
    .\install.ps1 -NonInteractive -CataclysmDir "C:\Sierra\Cataclysm" -RemasteredDir "C:\Program Files (x86)\Steam\steamapps\common\Homeworld"
#>
[CmdletBinding()]
param(
    # Folder holding cataclysm.big and cataclysm.exe.
    [string]$CataclysmDir,
    # Homeworld Remastered folder (steamapps\common\Homeworld). Optional.
    [string]$RemasteredDir,
    # Do not import the Homeworld Remastered effects, and do not ask.
    [switch]$NoRemastered,
    # Where the game goes. Default: the "game" folder next to this script.
    [string]$InstallDir,
    # Never show dialogs or prompts; fail with a message instead.
    [switch]$NonInteractive,
    # Use the binaries already in target\release.
    [switch]$SkipBuild,
    # Re-hash and parse-check every imported file afterwards (slower).
    [switch]$Verify,
    # Do not create a desktop shortcut.
    [switch]$NoShortcut,
    # Start the game when done.
    [switch]$Launch
)

$ErrorActionPreference = 'Stop'
$Repo = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $InstallDir) { $InstallDir = Join-Path $Repo 'game' }
$Interactive = -not $NonInteractive

function Say($msg) { Write-Host $msg -ForegroundColor Cyan }
function Fail($msg) {
    Write-Host "ERROR: $msg" -ForegroundColor Red
    if ($Interactive) { Read-Host 'Press Enter to close' | Out-Null }
    exit 1
}

if ($Interactive) { Add-Type -AssemblyName System.Windows.Forms }

function Ask-YesNo($text, $title) {
    $r = [System.Windows.Forms.MessageBox]::Show($text, $title, 'YesNo', 'Question')
    return $r -eq 'Yes'
}

function Pick-Folder($description, $start) {
    $d = New-Object System.Windows.Forms.FolderBrowserDialog
    $d.Description = $description
    $d.ShowNewFolderButton = $false
    if ($start -and (Test-Path $start)) { $d.SelectedPath = $start }
    if ($d.ShowDialog() -eq 'OK') { return $d.SelectedPath }
    return $null
}

# ---------- Locating the installs ----------

# Returns the folder holding cataclysm.big + cataclysm.exe at or just below $dir, else $null.
function Resolve-Cataclysm($dir) {
    if (-not $dir -or -not (Test-Path $dir)) { return $null }
    foreach ($d in @($dir, (Join-Path $dir 'Cataclysm'), (Join-Path $dir 'Homeworld Cataclysm'))) {
        if ((Test-Path (Join-Path $d 'cataclysm.big')) -and (Test-Path (Join-Path $d 'cataclysm.exe'))) {
            return (Resolve-Path $d).Path
        }
    }
    return $null
}

# Returns $dir if it is a Homeworld Remastered install (HomeworldRM\Data or Data with
# Homeworld2.big and HW1Ships.big), else $null.
function Resolve-Remastered($dir) {
    if (-not $dir -or -not (Test-Path $dir)) { return $null }
    foreach ($d in @((Join-Path $dir 'HomeworldRM\Data'), (Join-Path $dir 'Data'), $dir)) {
        if ((Test-Path (Join-Path $d 'Homeworld2.big')) -and (Test-Path (Join-Path $d 'HW1Ships.big'))) {
            return (Resolve-Path $dir).Path
        }
    }
    return $null
}

function Steam-Libraries {
    $roots = @()
    foreach ($k in 'HKCU:\Software\Valve\Steam', 'HKLM:\SOFTWARE\WOW6432Node\Valve\Steam') {
        try {
            $p = Get-ItemProperty $k -ErrorAction Stop
            if ($p.SteamPath) { $roots += $p.SteamPath }
            if ($p.InstallPath) { $roots += $p.InstallPath }
        } catch {}
    }
    $libs = @()
    foreach ($r in ($roots | Select-Object -Unique)) {
        $libs += $r
        $vdf = Join-Path $r 'steamapps\libraryfolders.vdf'
        if (Test-Path $vdf) {
            foreach ($m in [regex]::Matches((Get-Content $vdf -Raw), '"path"\s+"([^"]+)"')) {
                $libs += $m.Groups[1].Value -replace '\\\\', '\'
            }
        }
    }
    return $libs | Select-Object -Unique
}

function Find-Cataclysm {
    $candidates = @()
    foreach ($lib in Steam-Libraries) {
        $common = Join-Path $lib 'steamapps\common'
        if (Test-Path $common) {
            $candidates += Get-ChildItem $common -Directory -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match 'Cataclysm|Emergence' } | ForEach-Object { $_.FullName }
        }
    }
    foreach ($k in 'HKLM:\SOFTWARE\WOW6432Node\GOG.com\Games', 'HKLM:\SOFTWARE\GOG.com\Games') {
        if (Test-Path $k) {
            Get-ChildItem $k -ErrorAction SilentlyContinue | ForEach-Object {
                $g = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
                if ($g.gameName -match 'Cataclysm|Emergence' -and $g.path) { $candidates += $g.path }
            }
        }
    }
    foreach ($drive in (Get-PSDrive -PSProvider FileSystem | ForEach-Object { $_.Root })) {
        foreach ($rel in 'Games\Homeworld Cataclysm', 'Games\Homeworld Emergence', 'Sierra\Homeworld Cataclysm',
                         'Program Files (x86)\Sierra\Homeworld Cataclysm', 'Program Files (x86)\Sierra\Cataclysm',
                         'Program Files\Sierra\Homeworld Cataclysm', 'GOG Games\Homeworld Emergence',
                         'GOG Games\Homeworld Cataclysm', 'Sierra\Cataclysm') {
            $candidates += Join-Path $drive $rel
        }
    }
    foreach ($c in $candidates) { $r = Resolve-Cataclysm $c; if ($r) { return $r } }
    return $null
}

function Find-Remastered {
    foreach ($lib in Steam-Libraries) {
        $r = Resolve-Remastered (Join-Path $lib 'steamapps\common\Homeworld')
        if ($r) { return $r }
    }
    foreach ($k in 'HKLM:\SOFTWARE\WOW6432Node\GOG.com\Games', 'HKLM:\SOFTWARE\GOG.com\Games') {
        if (Test-Path $k) {
            foreach ($g in Get-ChildItem $k -ErrorAction SilentlyContinue) {
                $p = Get-ItemProperty $g.PSPath -ErrorAction SilentlyContinue
                if ($p.gameName -match 'Remastered' -and $p.path) {
                    $r = Resolve-Remastered $p.path
                    if ($r) { return $r }
                }
            }
        }
    }
    return $null
}

# ---------- 1. Rust ----------

Say '== Homeworld: Cataclysm (Rust) installer =='
if (-not $SkipBuild) {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo -and (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe")) {
        $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
        $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    }
    if (-not $cargo) {
        $msg = 'The Rust toolchain (cargo) is needed to build the game. Install it from https://rustup.rs'
        if ($Interactive -and (Get-Command winget -ErrorAction SilentlyContinue) -and
            (Ask-YesNo "Rust is not installed. Install it now with winget (Rustlang.Rustup)?`n`nRust also needs the Visual Studio C++ build tools; rustup offers to install them." 'Install Rust')) {
            winget install --id Rustlang.Rustup -e --accept-package-agreements --accept-source-agreements
            $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
            if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Fail "$msg (then run this installer again)." }
        } else {
            Fail $msg
        }
    }
}

# ---------- 2. Folders ----------

$hwc = Resolve-Cataclysm $CataclysmDir
if ($CataclysmDir -and -not $hwc) {
    Fail "No cataclysm.big and cataclysm.exe in '$CataclysmDir'. Point -CataclysmDir at the Homeworld: Cataclysm (or Emergence) install folder."
}
if (-not $hwc) {
    $found = Find-Cataclysm
    if ($NonInteractive) {
        if (-not $found) { Fail 'Homeworld: Cataclysm install not found. Pass -CataclysmDir "<folder with cataclysm.big>".' }
        $hwc = $found
    } else {
        $start = $found
        while (-not $hwc) {
            $pick = Pick-Folder 'Select your Homeworld: Cataclysm (or Emergence) install folder: the one with cataclysm.big and cataclysm.exe. It is only read.' $start
            if (-not $pick) { Fail 'No Homeworld: Cataclysm folder chosen.' }
            $hwc = Resolve-Cataclysm $pick
            if (-not $hwc) {
                [System.Windows.Forms.MessageBox]::Show("'$pick' has no cataclysm.big and cataclysm.exe. Please choose the game's install folder.", 'Not a Cataclysm install', 'OK', 'Warning') | Out-Null
                $start = $pick
            }
        }
    }
}
Say "Cataclysm install: $hwc"

$hwr = $null
if (-not $NoRemastered) {
    if ($RemasteredDir) {
        $hwr = Resolve-Remastered $RemasteredDir
        if (-not $hwr) { Fail "'$RemasteredDir' is not a Homeworld Remastered install (no HomeworldRM\Data\Homeworld2.big and HW1Ships.big)." }
    } elseif ($Interactive) {
        $found = Find-Remastered
        $q = "Optional: import the Homeworld 2 style explosions, engine trails and extra battle chatter from your own Homeworld Remastered install?`n`nThey are off by default in the game (Options > Enhancements). Skip this if you don't own Remastered."
        if ($found) { $q += "`n`nFound: $found" }
        if (Ask-YesNo $q 'Homeworld Remastered (optional)') {
            $start = $found
            while (-not $hwr) {
                $pick = Pick-Folder 'Select your Homeworld Remastered folder (steamapps\common\Homeworld). It is only read.' $start
                if (-not $pick) { break }
                $hwr = Resolve-Remastered $pick
                if (-not $hwr) {
                    [System.Windows.Forms.MessageBox]::Show("'$pick' is not a Homeworld Remastered install (needs HomeworldRM\Data\Homeworld2.big). Choose it again, or Cancel to skip.", 'Not a Remastered install', 'OK', 'Warning') | Out-Null
                    $start = $pick
                }
            }
        }
    }
}
if ($hwr) { Say "Remastered install: $hwr" } else { Say 'Remastered effects: skipped (optional)' }

# ---------- 3. Build ----------

if (-not $SkipBuild) {
    Say 'Building (release; the first build takes several minutes)...'
    Push-Location $Repo
    try {
        $ErrorActionPreference = 'Continue'  # cargo writes its progress to stderr
        cargo build --release -p cataclysm -p hwc-import
        $ErrorActionPreference = 'Stop'
        if ($LASTEXITCODE -ne 0) { Fail 'cargo build failed (see the messages above).' }
    } finally { Pop-Location }
}
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
$bin = Join-Path $targetDir 'release'
foreach ($exe in 'cataclysm.exe', 'hwc-import.exe') {
    if (-not (Test-Path (Join-Path $bin $exe))) { Fail "$exe is not in $bin. Run without -SkipBuild." }
}

# ---------- 4. Install and import ----------

New-Item -ItemType Directory -Force $InstallDir | Out-Null
$InstallDir = (Resolve-Path $InstallDir).Path
Copy-Item (Join-Path $bin 'cataclysm.exe'), (Join-Path $bin 'hwc-import.exe') $InstallDir -Force
$data = Join-Path $InstallDir 'data'

Say "Importing game files into $data ..."
$importArgs = @($hwc, $data)
if ($hwr) { $importArgs += @('--hwr', $hwr) }
if ($Verify) { $importArgs += '--verify' }
$ErrorActionPreference = 'Continue'  # the importer writes its progress to stderr
& (Join-Path $InstallDir 'hwc-import.exe') @importArgs
$ErrorActionPreference = 'Stop'
if ($LASTEXITCODE -ne 0) { Fail 'The import failed (see the messages above).' }

# ---------- 5. Launcher ----------

$cmd = Join-Path $InstallDir 'Play Cataclysm.cmd'
Set-Content -Path $cmd -Encoding ASCII -Value "@echo off`r`nstart `"`" /d `"%~dp0`" `"%~dp0cataclysm.exe`" `"%~dp0data`""

if (-not $NoShortcut) {
    try {
        $desktop = [Environment]::GetFolderPath('Desktop')
        $sc = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $desktop 'Homeworld Cataclysm (Rust).lnk'))
        $sc.TargetPath = Join-Path $InstallDir 'cataclysm.exe'
        $sc.Arguments = "`"$data`""
        $sc.WorkingDirectory = $InstallDir
        $sc.Save()
        Say 'Desktop shortcut created.'
    } catch { Write-Host "Could not create a desktop shortcut: $_" -ForegroundColor Yellow }
}

Say ''
Say "Done. Start the game with: `"$cmd`""
if ($Launch -or ($Interactive -and (Ask-YesNo 'Installed. Start the game now?' 'Homeworld: Cataclysm'))) {
    Start-Process -FilePath (Join-Path $InstallDir 'cataclysm.exe') -ArgumentList "`"$data`"" -WorkingDirectory $InstallDir
}
