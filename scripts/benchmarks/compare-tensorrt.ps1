param(
    [ValidateSet('cuda', 'cpu', 'webgpu', 'directml', 'trt-auto', 'trt-wide-fp32', 'trt-wide-fp16', 'trt-short-fp16')]
    [string]$Profile = 'cuda',
    [int]$Runs = 2,
    [string]$ModelDir = '.owlwhisp-test/models/parakeet-tdt-0.6b-v3',
    [string]$OutputDir = '.owlwhisp-test/tensorrt-study',
    [string]$RuntimeDir = 'runtime/win-x64'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$model = (Resolve-Path (Join-Path $root $ModelDir)).Path
$runtime = (Resolve-Path (Join-Path $root $RuntimeDir)).Path
$output = [IO.Path]::GetFullPath((Join-Path $root $OutputDir))
$profileDir = Join-Path $output $Profile
$linkedModel = Join-Path $profileDir 'model'
New-Item -ItemType Directory -Path $linkedModel -Force | Out-Null
# Share the original bytes; every profile gets its own engine cache without duplicating weights.
Get-ChildItem -LiteralPath $model -File | ForEach-Object {
    $link = Join-Path $linkedModel $_.Name
    if (-not (Test-Path -LiteralPath $link)) {
        New-Item -ItemType HardLink -Path $link -Target $_.FullName | Out-Null
    }
}

$options = @{}
if ($Profile.StartsWith('trt-')) {
    $backend = 'tensorrt'
    if ($Profile -ne 'trt-auto') {
        $min = 32
        $opt = 600
        $max = 2000
        if ($Profile -eq 'trt-short-fp16') { $min = 400; $opt = 640; $max = 800 }
        $options['ORT_TENSORRT_PROFILE_MIN_SHAPES'] = "audio_signal:1x128x$min,length:1"
        $options['ORT_TENSORRT_PROFILE_OPT_SHAPES'] = "audio_signal:1x128x$opt,length:1"
        $options['ORT_TENSORRT_PROFILE_MAX_SHAPES'] = "audio_signal:1x128x$max,length:1"
        $options['ORT_TENSORRT_FP16_ENABLE'] = if ($Profile.EndsWith('fp16')) { '1' } else { '0' }
    }
} else { $backend = $Profile }
$saved = @{}
try {
    foreach ($key in $options.Keys) {
        $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $options[$key], 'Process')
    }
    $options | ConvertTo-Json | Set-Content (Join-Path $profileDir 'options.json') -Encoding UTF8
    Push-Location $profileDir
    try {
        for ($run = 1; $run -le $Runs; $run++) {
            $timer = [Diagnostics.Stopwatch]::StartNew()
            $stdout = Join-Path $profileDir "run-$run.txt"
            $stderr = Join-Path $profileDir "run-$run.stderr.txt"
            $process = Start-Process -FilePath (Join-Path $root 'target/release/lw.exe') `
                -ArgumentList @('--runtime-dir', $runtime, 'bench', (Join-Path $root 'tests/fixtures/audio'),
                    '--model-dir', $linkedModel, '--backend', $backend) `
                -WorkingDirectory $profileDir -WindowStyle Hidden -PassThru -Wait `
                -RedirectStandardOutput $stdout -RedirectStandardError $stderr
            $process.WaitForExit()
            $timer.Stop()
            $record = [ordered]@{ profile = $Profile; run = $run; exit_code = $process.ExitCode;
                elapsed_seconds = $timer.Elapsed.TotalSeconds;
                cache_bytes = (Get-ChildItem -LiteralPath $linkedModel -Recurse -File |
                    Where-Object { $_.DirectoryName -like '*tensorrt-cache*' } |
                    Measure-Object -Property Length -Sum).Sum }
            $record | ConvertTo-Json | Set-Content (Join-Path $profileDir "run-$run.json") -Encoding UTF8
            $record | ConvertTo-Json -Compress
            Get-Content -LiteralPath $stdout | Select-String 'backend:|mean RTF:|warm RTF:'
            if ($process.ExitCode -ne 0) { Get-Content -LiteralPath $stderr | Select-Object -Last 10; break }
        }
    } finally { Pop-Location }
} finally {
    foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process') }
}
