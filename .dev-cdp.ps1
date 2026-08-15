param(
    [Parameter(Mandatory=$true)][string]$Match,     # 页面 URL 匹配关键字
    [Parameter(Mandatory=$true)][string]$Js,        # 要执行的 JS
    [int]$Port = 9223
)

$ErrorActionPreference = 'Stop'

# 查找目标页面
$pages = (Invoke-WebRequest -Uri "http://127.0.0.1:$Port/json/list" -UseBasicParsing -TimeoutSec 5).Content | ConvertFrom-Json
$page = $pages | Where-Object { $_.url -like "*$Match*" -and $_.type -eq 'page' } | Select-Object -First 1
if (-not $page) { Write-Output "PAGE_NOT_FOUND: $Match"; exit 1 }
$wsUrl = $page.webSocketDebuggerUrl

# WebSocket 连接
$ws = [System.Net.WebSockets.ClientWebSocket]::new()
$ws.ConnectAsync([Uri]$wsUrl, [Threading.CancellationToken]::None).Wait(5000) | Out-Null

function Send-Cdp($obj) {
    $json = $obj | ConvertTo-Json -Depth 10 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($json)
    $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, [Threading.CancellationToken]::None).Wait(5000) | Out-Null
}

function Recv-Cdp($wantId) {
    $buffer = New-Object byte[] 1048576
    for ($i = 0; $i -lt 200; $i++) {
        $ms = [System.IO.MemoryStream]::new()
        do {
            $res = $ws.ReceiveAsync([ArraySegment[byte]]::new($buffer), [Threading.CancellationToken]::None).Result
            $ms.Write($buffer, 0, $res.Count)
        } while (-not $res.EndOfMessage)
        $text = [Text.Encoding]::UTF8.GetString($ms.ToArray())
        $msg = $text | ConvertFrom-Json
        if ($msg.id -eq $wantId) { return $msg }
    }
    return $null
}

# 执行 JS
Send-Cdp @{ id = 1; method = 'Runtime.evaluate'; params = @{ expression = $Js; returnByValue = $true; awaitPromise = $true } }
$result = Recv-Cdp 1
if ($result.result.result.value -ne $null) {
    Write-Output $result.result.result.value
} elseif ($result.result.exceptionDetails) {
    Write-Output ("JS_ERROR: " + ($result.result.exceptionDetails.text))
} else {
    Write-Output ($result | ConvertTo-Json -Depth 10 -Compress)
}
$ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'done', [Threading.CancellationToken]::None).Wait(2000) | Out-Null
