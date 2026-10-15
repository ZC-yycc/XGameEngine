# Generate a short decaying sine "beep" as 16-bit mono PCM WAV.
# ASCII-only on purpose (repo rule: cmd/ps1 files stay ASCII).
param(
    [string]$OutPath = "D:\GitHubClone\XGameEngine\projects\SampleScene\Resources\Audio\Beep.wav"
)
$sr = 44100
$dur = 1.0
$n = [int]($sr * $dur)
$data = New-Object byte[] ($n * 2)
for ($i = 0; $i -lt $n; $i++) {
    $t = $i / $sr
    $env = [math]::Exp(-4.0 * $t)
    $s = [int16](0.4 * $env * [math]::Sin(2 * [math]::PI * 880.0 * $t) * 32767)
    $b = [BitConverter]::GetBytes($s)
    $data[$i * 2] = $b[0]
    $data[$i * 2 + 1] = $b[1]
}
$dir = Split-Path -Parent $OutPath
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([System.Text.Encoding]::ASCII.GetBytes("RIFF"))
$bw.Write([int](36 + $data.Length))
$bw.Write([System.Text.Encoding]::ASCII.GetBytes("WAVE"))
$bw.Write([System.Text.Encoding]::ASCII.GetBytes("fmt "))
$bw.Write([int]16)
$bw.Write([int16]1)          # PCM
$bw.Write([int16]1)          # mono
$bw.Write([int]$sr)
$bw.Write([int]($sr * 2))    # byte rate
$bw.Write([int16]2)          # block align
$bw.Write([int16]16)         # bits per sample
$bw.Write([System.Text.Encoding]::ASCII.GetBytes("data"))
$bw.Write([int]$data.Length)
$bw.Write($data)
$bw.Flush()
[System.IO.File]::WriteAllBytes($OutPath, $ms.ToArray())
Write-Output "written: $OutPath ($($data.Length) bytes of samples)"
