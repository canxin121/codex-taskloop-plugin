param(
  [ValidateSet("project", "user")]
  [string]$Scope = "project",
  [string]$Project = (Get-Location).Path,
  [string]$Name = "codex-taskloop-plugin",
  [string]$BinDir = "",
  [switch]$NoMcp,
  [switch]$NoHook,
  [switch]$NoBuild
)

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir "..")).Path

if ($BinDir -and -not (Test-Path $BinDir -PathType Container)) {
  Write-Error "Invalid -BinDir: $BinDir (expected a directory)"
  exit 1
}

function Resolve-BinInDir([string]$dir, [string]$name) {
  $exe = Join-Path $dir "$name.exe"
  if (Test-Path $exe) { return $exe }
  $plain = Join-Path $dir $name
  if (Test-Path $plain) { return $plain }
  return $null
}

function Find-BinSources([string]$overrideDir) {
  $candidates = @()
  if ($overrideDir) {
    $candidates += $overrideDir
  } else {
    $candidates += (Join-Path $RepoRoot "target\release")
    $candidates += (Join-Path $RepoRoot "bin")
  }
  foreach ($dir in $candidates) {
    if (-not (Test-Path $dir)) { continue }
    $mcp = Resolve-BinInDir $dir "codex-taskloop-plugin"
    $hook = Resolve-BinInDir $dir "codex-taskloop-plugin-hook"
    $admin = Resolve-BinInDir $dir "codex-taskloop-plugin-admin"
    if ($mcp -and $hook -and $admin) {
      return @{ Dir = $dir; Mcp = $mcp; Hook = $hook; Admin = $admin }
    }
  }
  return $null
}

$binSources = Find-BinSources $BinDir
if (-not $binSources) {
  if ($BinDir) {
    Write-Error "Binaries not found in $BinDir. Provide a valid -BinDir."
    exit 1
  }
  $cargoToml = Join-Path $RepoRoot "Cargo.toml"
  if (-not $NoBuild -and (Test-Path $cargoToml) -and (Get-Command cargo -ErrorAction SilentlyContinue)) {
    & cargo build --release --manifest-path $cargoToml
    $binSources = Find-BinSources ""
    if (-not $binSources) {
      Write-Error "Binaries not found after build; provide -BinDir."
      exit 1
    }
  } else {
    Write-Error "Binaries not found; provide -BinDir or build from source."
    exit 1
  }
}

$codexHomeDir = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $HOME ".codex" }
$projectBinDir = Join-Path $Project ".codex\bin"
$userBinDir = Join-Path $codexHomeDir "bin"
$binDestDir = if ($Scope -eq "project") { $projectBinDir } else { $userBinDir }
New-Item -ItemType Directory -Force $binDestDir | Out-Null

Copy-Item -Force $binSources.Mcp $binDestDir
Copy-Item -Force $binSources.Hook $binDestDir
Copy-Item -Force $binSources.Admin $binDestDir

$mcpBin = Join-Path $binDestDir (Split-Path -Leaf $binSources.Mcp)
$hookBin = Join-Path $binDestDir (Split-Path -Leaf $binSources.Hook)
$adminBin = Join-Path $binDestDir (Split-Path -Leaf $binSources.Admin)

if (-not (Test-Path $adminBin)) {
  Write-Error "admin binary not found at $adminBin; provide -BinDir"
  exit 1
}

if (-not $NoHook) {
  if ($Scope -eq "project") {
    & $adminBin hooks add --project $Project --command $hookBin
  } else {
    & $adminBin stop-hooks add --name $Name --command $hookBin
  }
}

if (-not $NoMcp) {
  $envArgs = @()
  if ($Scope -eq "project") {
    $envArgs += @("--env", "CODEX_CWD=$Project", "--env", "TASKLOOP_STORAGE_SCOPE=project-only")
  }
  if ($env:CODEX_HOME) {
    $envArgs += @("--env", "CODEX_HOME=$env:CODEX_HOME")
  }

  if (Get-Command codex -ErrorAction SilentlyContinue) {
    & codex mcp remove $Name | Out-Null
    & codex mcp add $Name @envArgs -- $mcpBin
  } else {
    if ($Scope -eq "project") {
      & $adminBin mcp add --name $Name --command $mcpBin --project $Project
    } else {
      & $adminBin mcp add --name $Name --command $mcpBin
    }
  }
}

$skillSourceDir = $null
$skillCandidates = @(
  (Join-Path $RepoRoot ".codex\skills\codex-taskloop-plugin"),
  (Join-Path $RepoRoot "skills\codex-taskloop-plugin")
)
foreach ($candidate in $skillCandidates) {
  if (Test-Path $candidate) {
    $skillSourceDir = $candidate
    break
  }
}
if (-not $skillSourceDir) {
  Write-Error "Skill source not found; expected .codex\\skills\\codex-taskloop-plugin or skills\\codex-taskloop-plugin"
  exit 1
}

if ($Scope -eq "project") {
  $projectSkillDir = Join-Path $Project ".codex\skills\codex-taskloop-plugin"
  New-Item -ItemType Directory -Force (Join-Path $Project ".codex\skills") | Out-Null
  if (Test-Path $projectSkillDir) { Remove-Item -Recurse -Force $projectSkillDir }
  Copy-Item -Recurse $skillSourceDir $projectSkillDir
  Write-Host "Installed codex-taskloop-plugin (project-level MCP + Stop hook). Project: $Project | MCP: $Name | Hook: $hookBin | Bin: $binDestDir | Skill: $projectSkillDir"
} else {
  $userSkillDir = Join-Path $codexHomeDir "skills\codex-taskloop-plugin"
  New-Item -ItemType Directory -Force (Join-Path $codexHomeDir "skills") | Out-Null
  if (Test-Path $userSkillDir) { Remove-Item -Recurse -Force $userSkillDir }
  Copy-Item -Recurse $skillSourceDir $userSkillDir
  Write-Host "Installed codex-taskloop-plugin (user-level MCP + Stop hook). MCP: $Name | Hook: $hookBin | Bin: $binDestDir | Skill: $userSkillDir"
}
