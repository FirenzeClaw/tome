# 进程 TCP 连接监控器（T006 无网络外发验证用）
#
# 完整说明见 BASELINE.md §2.3。本脚本刻意随结论一起入仓库：
# 结论若不可复现，就等于没有证据。
#
# 用法：pwsh -NoProfile -File monitor-process-network.ps1 -TargetPid <pid> -Seconds <n>
# 判定：末行 TOTAL_DISTINCT=0 表示该进程零 TCP 连接（含 SYN 尝试）。
#
# 注意：一个"永远输出 0"的监控器会伪造出"无外发"的假象。测目标进程之前
# 应先对**已知会联网**的进程跑一次（如 curl），确认它确实能检出连接。
# 实测对 curl 检出 4 条连接。

param(
    [Parameter(Mandatory = $true)][int]$TargetPid,
    [int]$Seconds = 1
)

$seen = @{}
$deadline = (Get-Date).AddSeconds($Seconds)

while ((Get-Date) -lt $deadline) {
    $connections = Get-NetTCPConnection -OwningProcess $TargetPid -ErrorAction SilentlyContinue
    foreach ($connection in $connections) {
        # 键含 State：一个 (远端地址, 端口) 从 SynSent 到 Established 的迁移
        # 是同一连接的进程，分别记录才能反映"真的建立了连接"而非仅尝试。
        $key = "$($connection.State)|$($connection.RemoteAddress):$($connection.RemotePort)"
        if (-not $seen.ContainsKey($key)) {
            $seen[$key] = $true
            Write-Output "CONN $key"
        }
    }
    # 5ms 采样：headless 路径总时长约 100ms，间隔过大可能整体错过。
    Start-Sleep -Milliseconds 5
}

Write-Output "TOTAL_DISTINCT=$($seen.Count)"
