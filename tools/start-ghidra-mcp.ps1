# Starts the ghidra-mcp headless server on 127.0.0.1:8089 against the HWC Ghidra project.
# The bridge (bridge-mcp-ghidra) talks to this; see .mcp.json at the repo root.
param(
    [int]$Port = 8089,
    [string]$Heap = "6g",
    # Enables /run_script_inline and /run_ghidra_script (arbitrary Java in the server); local use only.
    [switch]$AllowScripts
)
if ($AllowScripts) { $env:GHIDRA_MCP_ALLOW_SCRIPTS = "1" }
$root   = Split-Path -Parent $PSScriptRoot
$ghidra = Join-Path $PSScriptRoot "ghidra_12.1.4_PUBLIC"
$jar    = Join-Path $PSScriptRoot "ghidra-mcp\build\libs\GhidraMCP-7.0.0.jar"

$cp = @($jar)
foreach ($group in "Framework", "Features", "Debug", "Processors") {
    $cp += Get-ChildItem (Join-Path $ghidra "Ghidra\$group\*\lib\*.jar") | ForEach-Object FullName
}
$env:GHIDRA_MCP_FILE_ROOT = $root
& java "-Xmx$Heap" -XX:+UseG1GC -cp ($cp -join ";") com.xebyte.headless.GhidraMCPHeadlessServer `
    --bind 127.0.0.1 --port $Port --project (Join-Path $root "ghidra\HWC.gpr") --program cataclysm.exe
