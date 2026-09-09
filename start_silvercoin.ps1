# start_silvercoin.ps1
Write-Host "=== Starting Silvercoin Regtest MWEB Environment ===" -ForegroundColor Cyan

# 1. Stop existing node & miner processes to prevent port/mempool locks
Write-Host "[1/4] Terminating existing litecoind and miner instances..." -ForegroundColor Yellow
Get-Process -Name "litecoind", "silvercoin-miner" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 1

# 2. Locate litecoind.exe executable
$currentDir = Get-Location
$parentDir = Split-Path -Parent $currentDir
$litecoindPath = Join-Path $currentDir "litecoind.exe"

if (-not (Test-Path $litecoindPath)) {
    $litecoindPath = Join-Path $parentDir "litecoind.exe"
}

if (-not (Test-Path $litecoindPath)) {
    Write-Host "ERROR: Could not find litecoind.exe in current or parent folder!" -ForegroundColor Red
    Read-Host "Press ENTER to exit..."
    exit
}

Write-Host "[2/4] Found litecoind.exe at: $litecoindPath" -ForegroundColor Green

# 3. Generate litecoin.conf with proper RPC and MWEB parameters
$configDir = "$env:APPDATA\Litecoin"
if (-not (Test-Path $configDir)) {
    New-Item -ItemType Directory -Path $configDir | Out-Null
}

$configPath = Join-Path $configDir "litecoin.conf"
$configLines = @(
    "regtest=1",
    "server=1",
    "rpcuser=rtuser",
    "rpcpassword=rtpass",
    "rpcport=19332",
    "rpcallowip=127.0.0.1",
    "fallbackfee=0.00001",
    "mwebactivationheight=1"
)

$configLines | Set-Content -Path $configPath -Encoding UTF8
Write-Host "[3/4] Wrote regtest RPC configuration to: $configPath" -ForegroundColor Green

# 4. Launch litecoind daemon with immediate MWEB activation and clean mempool
Write-Host "[4/4] Launching litecoind daemon..." -ForegroundColor Yellow
Start-Process -FilePath $litecoindPath -ArgumentList "-regtest -mweb=1 -mwebactivationheight=1 -clearmempool" -WindowStyle Hidden

# 5. Wait for node RPC to respond before starting miner
Write-Host "Waiting for node RPC to initialize..." -ForegroundColor Cyan
$rpcReady = $false
$authHeader = "Basic " + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("rtuser:rtpass"))

for ($i = 0; $i -lt 15; $i++) {
    Start-Sleep -Seconds 1
    try {
        $body = '{"jsonrpc":"1.0","id":"test","method":"getblockchaininfo","params":[]}'
        $response = Invoke-RestMethod -Uri "http://127.0.0.1:19332" -Method Post -Headers @{ Authorization = $authHeader } -Body $body -ContentType "application/json" -ErrorAction Stop
        if ($response.result) {
            $rpcReady = $true
            break
        }
    } catch {
        # Node still starting up
    }
}

if ($rpcReady) {
    Write-Host "RPC online! Node active on block height: $($response.result.blocks)" -ForegroundColor Green
    Write-Host "Launching Rust miner..." -ForegroundColor Cyan
    cargo run --release
} else {
    Write-Host "ERROR: litecoind RPC failed to respond within 15 seconds." -ForegroundColor Red
}
