param([double]$BudgetGB = 3)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$rows = foreach ($entry in Get-ChildItem -LiteralPath $root -Force) {
    $bytes = if ($entry.PSIsContainer) {
        (Get-ChildItem -LiteralPath $entry.FullName -File -Recurse -Force |
            Measure-Object -Property Length -Sum).Sum
    } else { $entry.Length }
    [pscustomobject]@{ Name = $entry.Name; Bytes = [long]$bytes }
}
$total = ($rows | Measure-Object -Property Bytes -Sum).Sum
$rows | Sort-Object Bytes -Descending | Select-Object -First 8 |
    Select-Object Name, @{Name='GB';Expression={[math]::Round($_.Bytes / 1e9, 3)}} |
    Format-Table -AutoSize
Write-Host ('Workspace: {0:F3} GB / {1:F1} GB budget (decimal GB)' -f ($total / 1e9), $BudgetGB)
if ($total -gt $BudgetGB * 1e9) {
    Write-Warning 'Budget exceeded. Inspect build caches before the next build. No files were deleted.'
    exit 2
}
exit 0
