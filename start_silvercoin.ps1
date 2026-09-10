Write-Host "=== Starting Silvercoin Regtest MWEB Environment ===" -ForegroundColor Cyan

# -----------------------------
# 1. Kill existing processes
# -----------------------------
$targets = "litecoind", "silvercoin-miner"
Get-Process -Name $targets -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

# -----------------------------
# 2. Resolve litecoind.exe path
# -----------------------------
$currentDir = Get-Location
$parentDir  = Split-Path -Parent $currentDir

$litecoindPath = @(
    Join-Path $currentDir "litecoind.exe"
    Join-Path $parentDir  "litecoind.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $litecoindPath) {
    Write-Host "ERROR: litecoind.exe not found in current or parent directory." -ForegroundColor Red
    exit 1
}

# -----------------------------
# 3. Write clean config
# -----------------------------
$configDir  = "$env:APPDATA\Litecoin"
$configPath = Join-Path $configDir "litecoin.conf"

if (-not (Test-Path $configDir)) {
    New-Item -ItemType Directory -Path $configDir | Out-Null
}

@"
regtest=1
server=1
rpcuser=rtuser
rpcpassword=rtpass
rpcport=19332
rpcallowip=127.0.0.1
fallbackfee=0.00001
mwebactivationheight=1
txindex=1
blockfilterindex=1
deprecatedrpc=accounts
"@ | Set-Content -Path $configPath -Encoding UTF8

# -----------------------------
# 4. Launch daemon
# -----------------------------
Start-Process -FilePath $litecoindPath -ArgumentList `
    "-regtest -server -rpcuser=rtuser -rpcpassword=rtpass -rpcbind=127.0.0.1 -rpcallowip=127.0.0.1 -rpcport=19332 -txindex=1 -blockfilterindex=1 -deprecatedrpc=accounts -mwebactivationheight=1"

# -----------------------------
# 5. Poll RPC
# -----------------------------
$authHeader = "Basic " + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("rtuser:rtpass"))
$rpcReady   = $false
$rpcUrl     = "http://127.0.0.1:19332"
$payload    = '{"jsonrpc":"1.0","id":"test","method":"getblockchaininfo","params":[]}'

for ($i = 1; $i -le 15; $i++) {
    Start-Sleep -Milliseconds 700
    try {
        $response = Invoke-RestMethod -Uri $rpcUrl -Method Post -Headers @{ Authorization = $authHeader } -Body $payload -ContentType "application/json"
        if ($response.result.chain -eq "regtest") {
            $rpcReady = $true
            break
        }
    } catch {
        # ignore until timeout
    }
}

# -----------------------------
# 6. Start miner
# -----------------------------
if ($rpcReady) {
    Write-Host "RPC online! Chain: $($response.result.chain) | Height: $($response.result.blocks)" -ForegroundColor Green
    cargo run --release
} else {
    Write-Host "ERROR: litecoind RPC did not respond within timeout." -ForegroundColor Red
}
