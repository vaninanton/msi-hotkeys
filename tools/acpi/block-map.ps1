# Карта окон WMI: берём первые N полей из тела метода WQ<id>, где N — число
# экземпляров блока. Совпадение длины с числом экземпляров и есть проверка:
# WQAA даёт ровно 34 имени для MSI_Software, WQAB — 4 для MSI_Device и так далее.
$aml = [IO.File]::ReadAllBytes((Join-Path $PSScriptRoot 'dsdt.aml'))
$fields = @{}
foreach ($row in Import-Csv (Join-Path $PSScriptRoot 'ec-fields.csv')) { $fields[$row.Name] = $row }

$blocks = @(
    @{ Id = 'AA'; Name = 'MSI_Software'; Count = 34 }
    @{ Id = 'AB'; Name = 'MSI_Device';   Count = 4  }
    @{ Id = 'AC'; Name = 'MSI_Power';    Count = 3  }
    @{ Id = 'AD'; Name = 'MSI_Master_Battery'; Count = 16 }
    @{ Id = 'AE'; Name = 'MSI_Slave_Battery';  Count = 14 }
    @{ Id = 'AF'; Name = 'MSI_CPU';      Count = 19 }
    @{ Id = 'AG'; Name = 'MSI_VGA';      Count = 18 }
    @{ Id = 'AH'; Name = 'MSI_System';   Count = 21 }
    @{ Id = 'AI'; Name = 'MSI_AP';       Count = 8  }
)

function Find-At([byte[]]$b, [string]$s, [int]$from = 0) {
    $n = [byte[]][char[]]$s
    for ($i = $from; $i -lt $b.Length - $n.Length; $i++) {
        $ok = $true
        for ($j = 0; $j -lt $n.Length; $j++) { if ($b[$i+$j] -ne $n[$j]) { $ok = $false; break } }
        if ($ok) { return $i }
    }
    -1
}

$rows = @()
foreach ($block in $blocks) {
    $at = Find-At $aml ("WQ" + $block.Id)
    if ($at -lt 0) { "{0}: метод WQ{1} не найден" -f $block.Name, $block.Id; continue }

    $names = [System.Collections.ArrayList]::new()
    for ($i = $at + 4; $i -lt $aml.Length - 4 -and $names.Count -lt $block.Count; $i++) {
        $seg = -join (0..3 | ForEach-Object { [char]$aml[$i + $_] })
        if ($seg -notmatch '^[A-Z0-9_]{4}$') { continue }
        if (-not $fields.ContainsKey($seg)) { continue }
        if ($names -contains $seg) { continue }
        [void]$names.Add($seg)
    }

    ''
    "=== {0} ({1} экземпляров) ===" -f $block.Name, $block.Count
    $index = 0
    foreach ($n in $names) {
        $f = $fields[$n]
        $where = if ([int]$f.Width -ge 8) { $f.Byte } else { '{0} бит {1}' -f $f.Byte, $f.Bit }
        '  [{0,2}] {1,-5} EC {2,-12} ширина {3}' -f $index, $n, $where, $f.Width
        $rows += [pscustomobject]@{ Block = $block.Name; Index = $index; Field = $n; Ec = $f.Byte; Bit = $f.Bit; Width = $f.Width }
        $index++
    }
}

$rows | Export-Csv -Path (Join-Path $PSScriptRoot 'block-map.csv') -NoTypeInformation -Encoding utf8
''
"сохранено: block-map.csv, строк {0}" -f $rows.Count
