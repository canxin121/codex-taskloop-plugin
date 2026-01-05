param(
  [ValidateSet("project", "user")]
  [string]$Scope = "project",
  [string]$Project = (Get-Location).Path,
  [string]$Name = "codex-taskloop",
  [string]$BinDir = "",
  [switch]$NoMcp,
  [switch]$NoHook
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

$codexHomeDir = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $HOME ".codex" }
$projectSkillDir = Join-Path $Project ".codex\skills\codex-taskloop"
$userSkillDir = Join-Path $codexHomeDir "skills\codex-taskloop"
$projectBinDir = Join-Path $Project ".codex\bin"
$userBinDir = Join-Path $codexHomeDir "bin"
$binDestDir = if ($BinDir) { $BinDir } elseif ($Scope -eq "project") { $projectBinDir } else { $userBinDir }

$hookBin = Resolve-BinInDir $binDestDir "codex-taskloop-hook"
$adminBin = Resolve-BinInDir $binDestDir "codex-taskloop-admin"

if (-not $NoHook) {
  if (-not $adminBin) {
    Write-Error "admin binary not found in $binDestDir; provide -BinDir or use -NoHook"
    exit 1
  }
  if ($Scope -eq "project") {
    if (-not $hookBin) {
      Write-Error "hook binary not found in $binDestDir; provide -BinDir or use -NoHook"
      exit 1
    }
    & $adminBin hooks remove --project $Project --command $hookBin
  } else {
    & $adminBin stop-hooks remove --name $Name
  }
}

if ($Scope -eq "project") {
  $codexDir = Join-Path $Project ".codex"
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $codexDir "task-loop*.local.md")
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $codexDir "task-loop*.history.jsonl")

  $taskLoopDir = Join-Path $codexDir "task_loop"
  if (Test-Path $taskLoopDir) { Remove-Item -Recurse -Force $taskLoopDir }

  if (Test-Path $projectSkillDir) { Remove-Item -Recurse -Force $projectSkillDir }
} else {
  if (Test-Path $userSkillDir) { Remove-Item -Recurse -Force $userSkillDir }
}

if (-not $NoMcp) {
  if (Get-Command codex -ErrorAction SilentlyContinue) {
    & codex mcp remove $Name | Out-Null
  } else {
    if (-not $adminBin) {
      Write-Error "admin binary not found in $binDestDir; provide -BinDir or use -NoMcp"
      exit 1
    }
    & $adminBin mcp remove --name $Name
  }
}

if (Test-Path $binDestDir) {
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $binDestDir "codex-taskloop")
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $binDestDir "codex-taskloop.exe")
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $binDestDir "codex-taskloop-hook")
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $binDestDir "codex-taskloop-hook.exe")
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $binDestDir "codex-taskloop-admin")
  Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $binDestDir "codex-taskloop-admin.exe")
}

if ($Scope -eq "project") {
  Write-Host "Uninstalled codex-taskloop (project-level MCP + Stop hook). Project: $Project | Bin: $binDestDir | Skill: $projectSkillDir"
} else {
  Write-Host "Uninstalled codex-taskloop (user-level MCP + Stop hook). MCP: $Name | Bin: $binDestDir | Skill: $userSkillDir"
}
