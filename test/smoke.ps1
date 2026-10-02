# CI smoke: real login + list against OpenList. Credentials from repo secrets.
$ErrorActionPreference = 'Stop'
$url  = $env:MISC_URL
$user = $env:MISC_USER
$pass = $env:MISC_PASS
if (-not $url -or -not $user -or -not $pass) { throw "secrets MISC_URL/MISC_USER/MISC_PASS not set" }

$body = @{ username = $user; password = $pass } | ConvertTo-Json
$login = Invoke-RestMethod -Uri "$url/api/auth/login" -Method Post -Body $body -ContentType 'application/json'
if ($login.code -ne 200) { throw "login failed: $($login.message)" }
Write-Host "smoke: login OK"

$tok = $login.data.token
$list = Invoke-RestMethod -Uri "$url/api/fs/list" -Method Post `
  -Headers @{ Authorization = $tok } -ContentType 'application/json' `
  -Body (@{ path = '/'; page = 1; per_page = 5 } | ConvertTo-Json)
if ($list.code -ne 200) { throw "list failed: $($list.message)" }
Write-Host "smoke: list OK (root entries: $($list.data.content.Count))"
Write-Host "SMOKE PASS"
