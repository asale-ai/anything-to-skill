<#
.SYNOPSIS
    Install anything-to-skill on Windows.

.DESCRIPTION
    irm https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/install.ps1 | iex

    Options, as parameters when the script is run from a file, or as environment
    variables when it is piped into iex (which has no way to pass parameters):

      -Version <v>          VERSION                release to install, v0.2.0 or 0.2.0
                                                   (default: latest release)
      -BinDir <path>        BIN_DIR                where to put the binary
                                                   (default: ~\.local\bin)
      -Skill                SKILL=1                install the skill too
                                                   (default: binary only)
      -SkillDir <path>      SKILL_DIR              where to put the skill
                                                   (default: ~\.agents\skills)
      -AddToPath            ADD_TO_PATH=1          add BinDir to the user PATH
      -InsecureSkipVerify   INSECURE_SKIP_VERIFY=1 install without checking the
                                                   download (unsafe)

    The download is verified against the release's published SHA256 before it is
    installed. Verification failing and the checksum file being unreachable are
    both fatal — nothing is written in either. -InsecureSkipVerify installs
    without the check anyway (unsafe: it leaves you trusting the network).

.EXAMPLE
    $env:SKILL = '1'; irm https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/install.ps1 | iex

.EXAMPLE
    .\install.ps1 -Version 0.2.0 -Skill -AddToPath
#>

param(
    [string] $Version,
    [string] $BinDir,
    [string] $SkillDir,
    [switch] $Skill,
    [switch] $AddToPath,
    [switch] $InsecureSkipVerify
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Repo = 'asale-ai/anything-to-skill'
$Bin  = 'anything-to-skill'

function Say([string] $Message) { Write-Host $Message }

function Install-AnythingToSkill {
    # Expand-Archive and Get-FileHash are what set the floor; 5.1 ships with
    # Windows 10 and later, so this only trips on genuinely old machines.
    if ($PSVersionTable.PSVersion.Major -lt 5) {
        throw "this installer needs PowerShell 5.1 or newer (found $($PSVersionTable.PSVersion))"
    }

    # Fall back to the environment for every option, so the piped-into-iex form
    # can be configured at all.
    if (-not $Version)  { $Version  = $env:VERSION }
    if (-not $BinDir)   { $BinDir   = $env:BIN_DIR }
    if (-not $SkillDir) { $SkillDir = $env:SKILL_DIR }
    if (-not $Skill              -and $env:SKILL                -eq '1') { $Skill              = $true }
    if (-not $AddToPath          -and $env:ADD_TO_PATH          -eq '1') { $AddToPath          = $true }
    if (-not $InsecureSkipVerify -and $env:INSECURE_SKIP_VERIFY -eq '1') { $InsecureSkipVerify = $true }

    if (-not $BinDir)   { $BinDir   = Join-Path $HOME '.local\bin' }
    if (-not $SkillDir) { $SkillDir = Join-Path $HOME '.agents\skills' }

    # Windows PowerShell defaults to whatever SecurityProtocol the machine was
    # configured with, which on older installs still includes TLS 1.0. GitHub
    # refuses those, so the download would fail with a confusing reset.
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        # PowerShell 7 manages this itself and the type may not be settable.
    }

    # Invoke-WebRequest renders a progress bar by writing to the host on every
    # chunk, which makes a large download several times slower than it needs to
    # be. Restored in the finally below.
    $previousProgress = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('anything-to-skill-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null

    try {
        # ------------------------------------------------------- platform ---

        # A 32-bit PowerShell on 64-bit Windows reports x86 here and puts the
        # real answer in the WOW64 variable, so ask that one first.
        $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
        switch ($arch) {
            'AMD64' { $archPart = 'x86_64' }
            'ARM64' {
                # There is no aarch64-pc-windows-msvc build yet. Windows 11 on
                # ARM runs x64 binaries under emulation, so the x64 archive is a
                # working answer rather than a failure — say so and carry on.
                $archPart = 'x86_64'
                Say 'No native ARM64 build yet — installing the x64 one, which Windows on ARM runs under emulation.'
            }
            default { throw "unsupported architecture: $arch" }
        }
        $target = "$archPart-pc-windows-msvc"

        # -------------------------------------------------------- version ---

        if (-not $Version) {
            Say 'Finding the latest release...'
            # Read defensively: an API error (a rate limit, most likely) answers
            # with well-formed JSON that simply has no tag in it.
            $tag = $null
            try {
                $release = ConvertFrom-Json (Get-Text "https://api.github.com/repos/$Repo/releases/latest")
                if ($release.PSObject.Properties['tag_name']) { $tag = $release.tag_name }
            } catch {
                # Reported as the missing-release error below, which says what
                # to do about it.
            }
            if (-not $tag) {
                throw "could not determine the latest release`nSet the version explicitly, e.g. -Version v0.1.0 or `$env:VERSION = 'v0.1.0'"
            }
            $Version = $tag
        } elseif (-not $Version.StartsWith('v')) {
            # Releases are tagged "vX.Y.Z" while the archives are named "X.Y.Z",
            # so a bare 0.1.0 is the obvious thing to type. Accept both.
            $Version = "v$Version"
        }
        $number = $Version -replace '^v', ''

        $name = "$Bin-$number-$target"
        $zip  = Join-Path $tmp "$name.zip"
        $url  = "https://github.com/$Repo/releases/download/$Version/$name.zip"

        # ------------------------------------------------------- download ---

        Say "Downloading $name ..."
        try {
            Get-File $url $zip
        } catch {
            throw "download failed: $url`nCheck that a release for $target exists at https://github.com/$Repo/releases"
        }

        # Verify before unpacking, so a tampered archive is never even extracted.
        if ($InsecureSkipVerify) {
            Say 'warning: skipping verification — installing without checking the download'
        } else {
            $sumsUrl = "https://github.com/$Repo/releases/download/$Version/SHA256SUMS"
            $sums = Join-Path $tmp 'SHA256SUMS'
            try {
                Get-File $sumsUrl $sums
            } catch {
                throw "could not download the checksums: $sumsUrl`nRefusing to install unverified. Retry, or pass -InsecureSkipVerify to bypass."
            }

            # Anchored, and the hash has to look like one. The filename is
            # escaped because it contains dots, which are regex wildcards.
            $pattern  = '^([0-9a-fA-F]{64})\s+' + [regex]::Escape("$name.zip") + '\s*$'
            $expected = $null
            foreach ($line in (Get-Content -LiteralPath $sums)) {
                $m = [regex]::Match($line, $pattern)
                if ($m.Success) { $expected = $m.Groups[1].Value.ToLower(); break }
            }
            if (-not $expected) {
                throw "no checksum published for $name.zip`nRefusing to install unverified. Pass -InsecureSkipVerify to bypass."
            }

            $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
            if ($actual -ne $expected) {
                throw "checksum mismatch — refusing to install`n  expected $expected`n  actual   $actual"
            }
            Say 'Checksum verified.'
        }

        # -------------------------------------------------------- install ---

        Expand-Archive -LiteralPath $zip -DestinationPath $tmp -Force
        $unpacked = Join-Path $tmp $name
        $exe = Join-Path $unpacked "$Bin.exe"
        if (-not (Test-Path -LiteralPath $exe)) { throw "archive did not contain $Bin.exe" }

        New-Item -ItemType Directory -Path $BinDir -Force | Out-Null

        # Stage next to the destination and rename over it, so an interrupted
        # install cannot leave a half-written binary. Windows refuses to
        # overwrite an executable that is running, so an existing copy is moved
        # aside first — the rename succeeds even while it is in use, and the
        # leftover is deleted on the next run once nothing holds it open.
        $dest   = Join-Path $BinDir "$Bin.exe"
        $staged = "$dest.new"
        $old    = "$dest.old"
        Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue

        Copy-Item -LiteralPath $exe -Destination $staged -Force
        if (Test-Path -LiteralPath $dest) {
            try {
                Move-Item -LiteralPath $dest -Destination $old -Force
            } catch {
                Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
                throw "could not replace $dest — it is probably running. Close it and try again."
            }
        }
        try {
            Move-Item -LiteralPath $staged -Destination $dest -Force
        } catch {
            # Put the previous version back rather than leaving nothing there.
            if (Test-Path -LiteralPath $old) { Move-Item -LiteralPath $old -Destination $dest -Force }
            Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
            throw "could not install to $dest — check permissions"
        }
        Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue

        Say ''
        Say "Installed $Bin $Version to $dest"

        # ---------------------------------------------------------- skill ---

        # Off unless asked for. This script's job is the binary, and writing
        # into an agent's configuration is not a thing to do to someone who only
        # asked for that.
        #
        # The skill goes to ~\.agents\skills\, the vendor-neutral path Codex,
        # Cursor, Gemini CLI, and opencode all read. Claude Code reads only
        # ~\.claude\skills\, so it gets a junction to the same directory rather
        # than a second copy that would then have to be kept in step.
        #
        # Both files come out of the archive verified above, so nothing here is
        # fetched a second time or trusted unchecked. `npx skills add` does all
        # of this and covers many more agents; this path exists for machines
        # without Node.
        if ($Skill) {
            $skillDest = Join-Path $SkillDir $Bin
            New-Item -ItemType Directory -Path $skillDest -Force | Out-Null
            # Copied file by file rather than replacing the directory: anything
            # else the user keeps in there is theirs, and an installer should
            # not eat it.
            foreach ($file in 'SKILL.md', 'LICENSE') {
                Copy-Item -LiteralPath (Join-Path $unpacked $file) -Destination (Join-Path $skillDest $file) -Force
            }
            # SKILL.md is a map and references/ is what it points at. Installing
            # one without the other leaves every link in the skill going nowhere.
            $refs = Join-Path $unpacked 'references'
            if (Test-Path -LiteralPath $refs) {
                $destRefs = Join-Path $skillDest 'references'
                Remove-Item -LiteralPath $destRefs -Recurse -Force -ErrorAction SilentlyContinue
                Copy-Item -LiteralPath $refs -Destination $destRefs -Recurse -Force
            }

            Say ''
            Say "Installed the skill to $skillDest"

            # Only for agents that are actually here — creating a configuration
            # directory for a tool this machine does not have is litter, not
            # support.
            $claude = Join-Path $HOME '.claude'
            if (Test-Path -LiteralPath $claude) {
                Install-ClaudeLink -Claude $claude -SkillDest $skillDest
            }
        }

        # ----------------------------------------------------------- PATH ---

        $onPath = ($env:PATH -split ';' | ForEach-Object { $_.TrimEnd('\') }) -contains $BinDir.TrimEnd('\')
        if (-not $onPath) {
            if ($AddToPath) {
                # Written through .NET rather than setx, which silently
                # truncates a PATH longer than 1024 characters.
                $user = [Environment]::GetEnvironmentVariable('Path', 'User')
                $updated = if ($user) { $user.TrimEnd(';') + ';' + $BinDir } else { $BinDir }
                [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
                $env:PATH = "$env:PATH;$BinDir"
                Say ''
                Say "Added $BinDir to your PATH. Open a new terminal for it to take effect elsewhere."
            } else {
                Say ''
                Say "$BinDir is not on your PATH. Add it:"
                Say "  `$env:ADD_TO_PATH = '1'  # then re-run this installer"
            }
        }

        Say ''
        Say 'Next:'
        Say "  $Bin check              # see what optional tools are available"
        Say "  $Bin extract book.pdf   # pull the text out of a document"
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        $ProgressPreference = $previousProgress
    }
}

# A junction, not a symlink: symlinks need Developer Mode or an elevated shell,
# junctions need neither and Claude Code follows both the same way. Failing to
# link is not fatal — the skill is installed either way — so this falls back to
# a plain copy and says what it did.
function Install-ClaudeLink([string] $Claude, [string] $SkillDest) {
    $skills = Join-Path $Claude 'skills'
    $link   = Join-Path $skills $Bin

    if (Test-Path -LiteralPath $link) {
        $item = Get-Item -LiteralPath $link -Force
        $isLink = ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
        if (-not $isLink) {
            Say "  $link already exists and is not a link — left alone"
            return
        }
        # Deleted through .NET, which removes the junction itself. Remove-Item
        # -Recurse has followed directory links into their target in some
        # PowerShell versions, which here would empty the real skill directory.
        try { [IO.Directory]::Delete($link) } catch {
            Say "  could not replace the link at $link — left alone"
            return
        }
    }

    try {
        New-Item -ItemType Directory -Path $skills -Force | Out-Null
        New-Item -ItemType Junction -Path $link -Target $SkillDest -ErrorAction Stop | Out-Null
        Say '  linked into ~\.claude\skills\ for Claude Code'
    } catch {
        try {
            New-Item -ItemType Directory -Path $link -Force | Out-Null
            foreach ($file in 'SKILL.md', 'LICENSE') {
                Copy-Item -LiteralPath (Join-Path $SkillDest $file) -Destination (Join-Path $link $file) -Force
            }
            $refs = Join-Path $SkillDest 'references'
            if (Test-Path -LiteralPath $refs) {
                Copy-Item -LiteralPath $refs -Destination (Join-Path $link 'references') -Recurse -Force
            }
            Say '  copied into ~\.claude\skills\ for Claude Code (could not link)'
        } catch {
            Say "  could not install into ~\.claude\skills\ — copy $SkillDest there by hand"
        }
    }
}

function Get-File([string] $Url, [string] $Path) {
    Invoke-WebRequest -Uri $Url -OutFile $Path -UseBasicParsing -MaximumRedirection 10 `
        -Headers @{ 'User-Agent' = 'anything-to-skill-installer' }
}

function Get-Text([string] $Url) {
    (Invoke-WebRequest -Uri $Url -UseBasicParsing -MaximumRedirection 10 `
        -Headers @{ 'User-Agent' = 'anything-to-skill-installer' }).Content
}

try {
    Install-AnythingToSkill
} catch {
    Write-Host ''
    Write-Host "error: $($_.Exception.Message)" -ForegroundColor Red
    # `exit` from a script file reports failure to whatever ran it. Piped into
    # iex there is no script to leave, and exiting would close the user's
    # console instead — so only the status is set.
    $fromFile = Get-Variable -Name PSCommandPath -ValueOnly -ErrorAction SilentlyContinue
    if ($fromFile) { exit 1 }
    $global:LASTEXITCODE = 1
}
