use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

pub struct TlsInfo {
    pub subject: String,
    pub issuer: String,
    pub serial: String,
    pub not_before: String,
    pub not_after: String,
    pub days_left: i64,
    pub fingerprint: String,
}

pub fn do_tls_probe(host: &str, port: u16) -> Result<TlsInfo, String> {
    use der::Decode;
    use x509_cert::Certificate;

    // Install crypto provider if not already installed
    let _ = rustls::crypto::ring::default_provider().install_default();

    let addr = format!("{}:{}", host, port);
    let mut tcp =
        TcpStream::connect(&addr).map_err(|e| format!("tls_probe: connection failed: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("tls_probe: {e}"))?;

    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();

    // Allow self-signed certs for monitoring purposes
    config
        .dangerous()
        .set_certificate_verifier(std::sync::Arc::new(Insecure));

    let rc_config = std::sync::Arc::new(config);
    let server_name = host
        .to_owned()
        .try_into()
        .map_err(|e| format!("tls_probe: invalid hostname: {e}"))?;
    let mut session = rustls::ClientConnection::new(rc_config, server_name)
        .map_err(|e| format!("tls_probe: {e}"))?;

    let mut tls = rustls::Stream::new(&mut session, &mut tcp);
    tls.flush()
        .map_err(|e| format!("tls_probe: handshake failed: {e}"))?;

    // Get the peer certificate
    let certs = session
        .peer_certificates()
        .ok_or_else(|| "tls_probe: no certificate presented".to_string())?;

    let cert_der = certs
        .first()
        .ok_or_else(|| "tls_probe: certificate chain is empty".to_string())?;

    let cert = Certificate::from_der(cert_der.as_ref())
        .map_err(|e| format!("tls_probe: failed to parse certificate: {e}"))?;

    let subject = cert.tbs_certificate.subject.to_string();
    let issuer = cert.tbs_certificate.issuer.to_string();
    let serial = cert.tbs_certificate.serial_number.to_string();

    // Validity dates
    let not_before = cert.tbs_certificate.validity.not_before.to_string();
    let not_after = cert.tbs_certificate.validity.not_after.to_string();

    // Calculate days until expiry
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiry_secs = cert
        .tbs_certificate
        .validity
        .not_after
        .to_system_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days_left = (expiry_secs as i64 - now as i64) / 86400;

    // Fingerprint using xxhash (already in deps)
    use xxhash_rust::xxh3::xxh3_64;
    let hash = xxh3_64(cert_der.as_ref());
    let fingerprint = format!("XXH3:{:016x}", hash);

    Ok(TlsInfo {
        subject,
        issuer,
        serial,
        not_before,
        not_after,
        days_left,
        fingerprint,
    })
}

// Allow self-signed certificates for monitoring
#[derive(Debug)]
struct Insecure;

impl rustls::client::danger::ServerCertVerifier for Insecure {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dgs: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dgs: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}
