//! TLS 1.3 options for the tunnel used by S7-1500 >= V2.9 and S7-1200 >=
//! V4.5 after InitSsl. rustls with the ring provider. The PLC
//! certificate is self-signed or issued by the TIA project CA: by default it
//! is not verified (the handshake signature is still checked).

use std::fmt;
use std::io::Write;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::error::{Error, Result};
use crate::textfmt::hex;

/// TLS options of a connection.
#[derive(Clone, Default)]
pub struct TlsOptions {
    /// None = TLS 1.3 with the ring provider and no certificate verification.
    pub config: Option<Arc<rustls::ClientConfig>>,
    /// Optional NSS key log (Wireshark), e.g. [`NssKeyLog`].
    pub key_log: Option<Arc<dyn rustls::KeyLog>>,
    /// Server name (SNI); empty = the host part of the address (an IP
    /// address sends no SNI).
    pub server_name: String,
}

impl fmt::Debug for TlsOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsOptions")
            .field("config", &self.config.is_some())
            .field("key_log", &self.key_log.is_some())
            .field("server_name", &self.server_name)
            .finish()
    }
}

/// The ring crypto provider.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Accepts any certificate (no chain or name check), still verifies the
/// handshake signature with the certificate's key.
#[derive(Debug)]
pub struct NoCertVerification(pub Arc<CryptoProvider>);

impl ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// NSS key log writer ("LABEL client_random secret" lines) for Wireshark.
pub struct NssKeyLog {
    out: Mutex<Box<dyn Write + Send>>,
}

impl NssKeyLog {
    pub fn new(w: impl Write + Send + 'static) -> Self {
        NssKeyLog { out: Mutex::new(Box::new(w)) }
    }

    /// Appends to a file (created with mode 0600 on Unix).
    pub fn file(path: &str) -> std::io::Result<Self> {
        let mut o = std::fs::OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        Ok(NssKeyLog::new(o.open(path)?))
    }
}

impl fmt::Debug for NssKeyLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NssKeyLog")
    }
}

impl rustls::KeyLog for NssKeyLog {
    fn log(&self, label: &str, client_random: &[u8], secret: &[u8]) {
        if let Ok(mut o) = self.out.lock() {
            let _ = writeln!(o, "{label} {} {}", hex(client_random), hex(secret));
            let _ = o.flush();
        }
    }
    fn will_log(&self, _label: &str) -> bool {
        true
    }
}

/// A shared in-memory buffer usable as key log target (tests, UIs).
#[derive(Clone, Default, Debug)]
pub struct SharedBuf(pub Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    pub fn contents(&self) -> Vec<u8> {
        self.0.lock().map(|b| b.clone()).unwrap_or_default()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(b);
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Builds the client configuration of a connection.
pub(crate) fn client_config(o: &TlsOptions) -> Result<Arc<rustls::ClientConfig>> {
    let mut cfg = match &o.config {
        Some(c) => (**c).clone(),
        None => {
            let p = provider();
            rustls::ClientConfig::builder_with_provider(p.clone())
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| Error::Tls(e.to_string()))?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoCertVerification(p)))
                .with_no_client_auth()
        }
    };
    if let Some(kl) = &o.key_log {
        cfg.key_log = kl.clone();
    }
    Ok(Arc::new(cfg))
}

/// The server name for a "host:port" address.
pub(crate) fn server_name(o: &TlsOptions, addr: &str) -> ServerName<'static> {
    let host = if !o.server_name.is_empty() {
        o.server_name.clone()
    } else {
        let h = match addr.rsplit_once(':') {
            Some((h, _)) => h,
            None => addr,
        };
        h.trim_start_matches('[').trim_end_matches(']').to_string()
    };
    ServerName::try_from(host).unwrap_or_else(|_| ServerName::try_from("plc".to_string()).unwrap())
}
