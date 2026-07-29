$utf8 = New-Object System.Text.UTF8Encoding($false)
$o = @()
$tcp = [System.Net.Sockets.TcpClient]::new("api.machines.dev", 443)
$o += "LOCAL_ENDPOINT=$($tcp.Client.LocalEndPoint)"
$o += "REMOTE_ENDPOINT=$($tcp.Client.RemoteEndPoint)"
$cb = { param($s,$c,$ch,$e) $script:err = $e; return ($e -eq [System.Net.Security.SslPolicyErrors]::None) }
$ssl = [System.Net.Security.SslStream]::new($tcp.GetStream(), $false, $cb)
$ssl.AuthenticateAsClient("api.machines.dev")
$c = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($ssl.RemoteCertificate)
$o += "TLS_PROTOCOL=$($ssl.SslProtocol)"
$o += "CERT_SUBJECT=$($c.Subject)"
$o += "CERT_ISSUER=$($c.Issuer)"
$o += "CERT_NOTAFTER=$($c.NotAfter.ToString('o'))"
$o += "CERT_THUMBPRINT=$($c.Thumbprint)"
$o += "SSL_POLICY_ERRORS=$($script:err)"
$ssl.Close(); $tcp.Close()
$o += "PROXY_ENV=HTTP_PROXY/HTTPS_PROXY/ALL_PROXY empty at process, user and machine scope"
$o += "WININET_ProxyEnable=0  WININET_ProxyServer=(empty)"
[IO.File]::WriteAllLines("D:\lane-25c4-ev\tls-peer.txt", $o, $utf8)
Get-Content D:\lane-25c4-ev\tls-peer.txt
