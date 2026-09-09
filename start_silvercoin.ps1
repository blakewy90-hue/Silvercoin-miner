Write-Host "=== Starting Silvercoin Regtest MWEB Environment ===" -ForegroundColor Cyan

# 1. Terminate existing instances
Get-Process -Name "litecoind", "silvercoin-miner" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 1

# 2. Locate litecoind.exe
$currentDir = Get-Location
$parentDir = Split-Path -Parent $currentDir
$litecoindPath = Join-Path $currentDir "litecoind.exe"
if (-not (Test-Path $litecoindPath)) { 
    $litecoindPath = Join-Path $parentDir "litecoind.exe" 
}

if (-not (Test-Path $litecoindPath)) {
    Write-Host "ERROR: Could not find litecoind.exe in $currentDir or $parentDir" -ForegroundColor Red
    exit
}

# 3. Write configuration
$configDir = "$env:APPDATA\Litecoin"
if (-not (Test-Path $configDir)) { 
    New-Item -ItemType Directory -Path $configDir | Out-Null 
}

$configPath = Join-Path $configDir "litecoin.conf"

@("regtest=1", "server=1", "rpcuser=rtuser", "rpcpassword=rtpass", "rpcport=19332", "rpcallowip=127.0.0.1", "fallbackfee=0.00001", "mwebactivationheight=1", "txindex=1", "blockfilterindex=1", "deprecatedrpc=accounts") | Set-Content -Path $configPath -Encoding UTF8

# 4. Launch daemon in a visible window
Start-Process -FilePath $litecoindPath -ArgumentList "-regtest -server -rpcuser=rtuser -rpcpassword=rtpass -rpcbind=127.0.0.1 -rpcallowip=127.0.0.1 -rpcport=19332 -txindex=1 -blockfilterindex=1 -deprecatedrpc=accounts -mwebactivationheight=1"

# 5. Poll RPC until confirmed on regtest
$rpcReady = $false
$authHeader = "Basic " + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("rtuser:rtpass"))

for ($i = 0; $i -lt 15; $i++) {
    Start-Sleep -Seconds 1
    try {
        $body = '{"jsonrpc":"1.0","id":"test","method":"getblockchaininfo","params":[]}'
        $response = Invoke-RestMethod -Uri "http://127.0.0.1:19332" -Method Post -Headers @{ Authorization = $authHeader } -Body $body -ContentType "application/json" -ErrorAction Stop
        if ($response.result -and $response.result.chain -eq "regtest") {
            $rpcReady = $true
            break
        }
    } catch {}
}

# 6. Run miner
if ($rpcReady) {
    Write-Host "RPC online! Chain: $($response.result.chain) | Height: $($response.result.blocks)" -ForegroundColor Green
    cargo run --release
} else {
    Write-Host "ERROR: litecoind failed to respond on RPC within 15 seconds." -ForegroundColor Red
}
