param([Parameter(Mandatory=$true)][string]$Binary)
$ErrorActionPreference = 'Stop'
$exe = (Resolve-Path $Binary).Path
foreach ($mode in @('headless', 'gui')) {
    $root = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString('n'))
    $process = $null
    try {
        & $exe init $root
        if ($LASTEXITCODE -ne 0) { throw 'Sample setup failed' }
        $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
        $listener.Start()
        $port = $listener.LocalEndpoint.Port
        $listener.Stop()
        $config = Join-Path $root 'vendredi.toml'
        [IO.File]::WriteAllText($config, ([IO.File]::ReadAllText($config)).Replace(':8180', ":$port"))
        $start = [Diagnostics.ProcessStartInfo]::new($exe)
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardInput = $true
        $start.RedirectStandardOutput = $true
        if ($mode -eq 'gui') { $start.ArgumentList.Add('gui') }
        else { $start.ArgumentList.Add('run'); $start.ArgumentList.Add('--headless') }
        $start.ArgumentList.Add($root)
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $process = [Diagnostics.Process]::Start($start)
        if ($mode -eq 'gui') {
            do {
                Start-Sleep -Milliseconds 10
                $process.Refresh()
                if ($process.HasExited -or $clock.Elapsed.TotalSeconds -gt 15) { throw 'Window failed to open' }
            } until ($process.MainWindowHandle -ne 0 -and $process.MainWindowTitle -eq 'Vendredi')
        } else {
            $ready = $process.StandardOutput.ReadLineAsync()
            if (-not $ready.Wait(15000) -or ($ready.Result | ConvertFrom-Json).event -ne 'ready') { throw 'Listener failed to start' }
        }
        $readyMs = $clock.ElapsedMilliseconds
        $result = Invoke-WebRequest -Method Post -Uri "http://127.0.0.1:$port/supplier/orders" -InFile (Join-Path $root 'order.edi') -ContentType application/edifact
        if ($result.StatusCode -ne 202) { throw 'Sample order failed' }
        $end = [DateTime]::UtcNow.AddSeconds(10)
        while (@(Get-ChildItem (Join-Path $root 'replies') -Filter '*.edi').Count -ne 2) {
            if ([DateTime]::UtcNow -gt $end) { throw 'Sample replies missing' }
            Start-Sleep -Milliseconds 10
        }
        Start-Sleep -Seconds 1
        $process.Refresh()
        $beginCpu = $process.TotalProcessorTime.TotalMilliseconds
        $firstRss = $process.WorkingSet64
        $idle = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds 5
        $process.Refresh()
        [pscustomobject]@{
            mode = $mode
            bytes = (Get-Item $exe).Length
            ready_ms = $readyMs
            rss_bytes = [Math]::Max($firstRss, $process.WorkingSet64)
            idle_cpu_percent = [Math]::Round(100 * ($process.TotalProcessorTime.TotalMilliseconds - $beginCpu) / $idle.ElapsedMilliseconds, 2)
        } | ConvertTo-Json
    } finally {
        if ($process -and -not $process.HasExited) {
            if ($mode -eq 'gui') { [void]$process.CloseMainWindow() } else { $process.StandardInput.WriteLine('quit') }
            if (-not $process.WaitForExit(5000)) { $process.Kill(); $process.WaitForExit() }
        }
        if (Test-Path $root) { Remove-Item -LiteralPath $root -Recurse -Force }
    }
}
