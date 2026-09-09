Write-Host "=== Silvercoin Miner Launcher ===" -ForegroundColor Cyan

# 1. Stop stale litecoind instances
Get-Process -Name "litecoind" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 1
Start-Process -FilePath "..\litecoind.exe" -ArgumentList "-regtest -mwebactivationheight=1 -clearmempool" -WindowStyle Hidden


# 2. Dynamic path detection
$currentDir = Get-Location
$parentDir = Split-Path -Path $currentDir -Parent

$litecoindPath = Join-Path $currentDir "litecoind.exe"
if (-not (Test-Path $litecoindPath)) {
    $litecoindPath = Join-Path $parentDir "litecoind.exe"
    }

    if (-not (Test-Path $litecoindPath)) {
        Write-Host "ERROR: Could not find litecoind.exe!" -ForegroundColor Red
            Write-Host "Searched in: $currentDir and $parentDir" -ForegroundColor Yellow
                Read-Host "Press ENTER to exit..."
                    exit
                    }

                    Write-Host "Found litecoind.exe at: $litecoindPath" -ForegroundColor Green

                    # 3. Write configuration for Regtest mode
                    $configDir = "$env:APPDATA\Litecoin"
                    if (-not (Test-Path $configDir)) { 
                        New-Item -ItemType Directory -Path $configDir | Out-Null 
                        }

                        $configLines = @(
                            "server=1",
                                "regtest=1",
                                    "rpcuser=admin",
                                        "rpcpassword=password123",
                                            "rpcallowip=127.0.0.1",
                                                "rpcport=19334",
                                                    "fallbackfee=0.00001",
                                                        "[regtest]",
                                                            "rpcport=19334"
                                                            )
                                                            $configLines | Set-Content -Path "$configDir\litecoin.conf" -Force
                                                            Write-Host "Config file updated." -ForegroundColor Green

                                                            # 4. Start daemon in a visible window
                                                            Write-Host "Starting Litecoin Daemon..." -ForegroundColor Green
                                                            Start-Process -FilePath $litecoindPath -ArgumentList "-regtest"

                                                            # 5. Poll RPC readiness
                                                            Write-Host "Connecting to Node RPC..." -ForegroundColor Yellow
                                                            $rpcReady = $false
                                                            $auth = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("admin:password123"))

                                                            for ($i = 1; $i -le 20; $i++) {
                                                                Start-Sleep -Seconds 1
                                                                    try {
                                                                            $body = '{"jsonrpc":"1.0","id":"check","method":"getblockchaininfo","params":[]}'
                                                                                    $res = Invoke-RestMethod -Uri "http://127.0.0.1:19334" -Method Post -Headers @{ Authorization = "Basic $auth" } -Body $body -ContentType "application/json" -ErrorAction Stop
                                                                                            if ($res) { 
                                                                                                        $rpcReady = $true
                                                                                                                    break 
                                                                                                                            }
                                                                                                                                } catch {
                                                                                                                                        Write-Host "." -NoNewline
                                                                                                                                            }
                                                                                                                                            }

                                                                                                                                            Write-Host ""
                                                                                                                                            if (-not $rpcReady) {
                                                                                                                                                Write-Host "ERROR: Litecoin RPC failed to respond after 20 seconds." -ForegroundColor Red
                                                                                                                                                    Read-Host "Press ENTER to exit..."
                                                                                                                                                        exit
                                                                                                                                                        }

                                                                                                                                                        Write-Host "RPC Connected!" -ForegroundColor Green

                                                                                                                                                        # 6. Locate Cargo directory and launch miner
                                                                                                                                                        $minerDir = $currentDir
                                                                                                                                                        if (-not (Test-Path (Join-Path $minerDir "Cargo.toml"))) {
                                                                                                                                                            $minerDir = Join-Path $currentDir "silvercoin-miner"
                                                                                                                                                            }

                                                                                                                                                            if (-not (Test-Path (Join-Path $minerDir "Cargo.toml"))) {
                                                                                                                                                                Write-Host "ERROR: Could not locate Cargo.toml folder!" -ForegroundColor Red
                                                                                                                                                                    Read-Host "Press ENTER to exit..."
                                                                                                                                                                        exit
                                                                                                                                                                        }

                                                                                                                                                                        Set-Location -Path $minerDir
                                                                                                                                                                        Write-Host "Running Rust Miner..." -ForegroundColor Green
                                                                                                                                                                        cargo run --release

                                                                                                                                                                        Read-Host "Process finished. Press ENTER to exit..."
                                                                                                                                                                        