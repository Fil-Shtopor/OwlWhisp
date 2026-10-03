# Read-only Windows sampler. Use the same duration/state for every client in a comparison.
param(
    [Parameter(Mandatory = $true)][int[]]$ProcessId,
    [ValidateRange(1, 3600)][int]$DurationSeconds = 60,
    [ValidateRange(200, 10000)][int]$IntervalMilliseconds = 1000,
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
$roots = @{}
foreach ($clientId in ($ProcessId | Select-Object -Unique)) {
    $process = Get-Process -Id $clientId
    $roots[$clientId] = [pscustomobject]@{
        process = $process
        started = $process.StartTime
        samples = [System.Collections.Generic.List[object]]::new()
    }
}
$timer = [System.Diagnostics.Stopwatch]::StartNew()
while ($timer.Elapsed.TotalSeconds -lt $DurationSeconds) {
    # One shared snapshot keeps clients comparable and avoids repeating WMI queries per client.
    $processes = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, CreationDate, Name)
    $counters = @{}
    foreach ($counter in (Get-CimInstance Win32_PerfRawData_PerfProc_Process)) {
        if ($counter.IDProcess -gt 0) { $counters[[int]$counter.IDProcess] = $counter }
    }
    foreach ($clientId in $roots.Keys) {
        $root = $roots[$clientId]
        $currentRoot = Get-Process -Id $clientId -ErrorAction SilentlyContinue
        if (-not $currentRoot -or $currentRoot.StartTime -ne $root.started) { continue }
        $ids = [System.Collections.Generic.HashSet[int]]::new()
        [void]$ids.Add($clientId)
        do {
            $added = $false
            foreach ($process in $processes) {
                if ($ids.Contains([int]$process.ParentProcessId) -and $process.CreationDate -ge $root.started) {
                    $added = $ids.Add([int]$process.ProcessId) -or $added
                }
            }
        } while ($added)
        $workingSet = 0L
        $privateWorkingSet = 0L
        $privateCommit = 0L
        $details = [System.Collections.Generic.List[object]]::new()
        $missing = 0
        foreach ($processIdToMeasure in $ids) {
            $counter = $counters[$processIdToMeasure]
            if (-not $counter) { $missing++; continue }
            $workingSet += [long]$counter.WorkingSet
            $privateWorkingSet += [long]$counter.WorkingSetPrivate
            $privateCommit += [long]$counter.PrivateBytes
            $details.Add([pscustomobject]@{
                process_id = $processIdToMeasure
                name = ($processes | Where-Object ProcessId -eq $processIdToMeasure | Select-Object -First 1).Name
                working_set_mib = [math]::Round($counter.WorkingSet / 1MB, 2)
                private_working_set_mib = [math]::Round($counter.WorkingSetPrivate / 1MB, 2)
                private_commit_mib = [math]::Round($counter.PrivateBytes / 1MB, 2)
            })
        }
        $root.samples.Add([pscustomobject]@{
            seconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
            process_count = $details.Count
            missing_counter_count = $missing
            working_set_mib = [math]::Round($workingSet / 1MB, 2)
            private_working_set_mib = $(if ($missing -eq 0) { [math]::Round($privateWorkingSet / 1MB, 2) } else { $null })
            private_commit_mib = [math]::Round($privateCommit / 1MB, 2)
            processes = @($details.ToArray())
        })
    }
    Start-Sleep -Milliseconds $IntervalMilliseconds
}
function Get-Median($values) {
    $sorted = @($values | Where-Object { $null -ne $_ } | Sort-Object)
    if ($sorted.Count -eq 0) { return $null }
    $middle = [int][math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2) { return $sorted[$middle] }
    return ($sorted[$middle - 1] + $sorted[$middle]) / 2
}
$clients = foreach ($clientId in ($roots.Keys | Sort-Object)) {
    $root = $roots[$clientId]
    $samples = $root.samples
    if ($samples.Count -eq 0) { throw "Client $clientId exited before the first sample." }
    [pscustomobject]@{
        measured_at_utc = [DateTime]::UtcNow.ToString('o')
        root_process_id = $clientId
        root_process_name = $root.process.ProcessName
        root_started_utc = $root.started.ToUniversalTime().ToString('o')
        duration_seconds = [math]::Round($timer.Elapsed.TotalSeconds, 2)
        sample_count = $samples.Count
        complete_private_working_set_sample_count = @($samples | Where-Object { $null -ne $_.private_working_set_mib }).Count
        includes_descendants = $true
        working_set_median_mib = [math]::Round((Get-Median $samples.working_set_mib), 2)
        working_set_peak_mib = ($samples.working_set_mib | Measure-Object -Maximum).Maximum
        private_working_set_median_mib = Get-Median $samples.private_working_set_mib
        private_working_set_peak_mib = ($samples.private_working_set_mib | Measure-Object -Maximum).Maximum
        private_commit_median_mib = [math]::Round((Get-Median $samples.private_commit_mib), 2)
        private_commit_peak_mib = ($samples.private_commit_mib | Measure-Object -Maximum).Maximum
        samples = @($samples.ToArray())
    }
}
$report = if ($roots.Count -eq 1) { $clients } else { [pscustomobject]@{clients = @($clients)} }
$json = $report | ConvertTo-Json -Depth 7
if ($OutputPath) { $json | Set-Content -LiteralPath $OutputPath -Encoding utf8 }
$json
