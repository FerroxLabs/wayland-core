$ErrorActionPreference='Continue'
$log='C:\f28e5\selftest.log'
$status='C:\f28e5\selftest.status'
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28E5 SELFTEST started=" + (Get-Date -Format o)) -Encoding utf8
function L($m){ Add-Content -Path $log -Value $m -Encoding utf8 }
L ("MJS_SHA256=" + (Get-FileHash -Algorithm SHA256 C:\f28e5\f28-native-matrix.mjs).Hash.ToLower())
L ("TSV_SHA256=" + (Get-FileHash -Algorithm SHA256 C:\f28e5\matrix.tsv).Hash.ToLower())
& node C:\f28e5\f28-native-matrix.mjs --self-test 2>&1 | ForEach-Object { L $_ }
$rc = $LASTEXITCODE
L ("SELFTEST_RC=" + $rc)
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
exit $rc
