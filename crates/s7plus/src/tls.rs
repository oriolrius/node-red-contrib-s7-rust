//! TLS 1.3 options for the tunnel used by S7-1500 >= V2.9 and S7-1200 >=
//! V4.5 after InitSsl. rustls with the ring provider. The PLC
//! certificate is self-signed or issued by the TIA project CA: by default it
//! is not verified (the handshake signature is still checked). [`CertVerify`]
//! pins its SHA-256 fingerprint or checks it against trusted certificates.

use std::fmt;
use std::io::Write;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{verify_server_cert_signed_by_trust_anchor, verify_server_name};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{CertificateError, DigitallySignedStruct, RootCertStore, SignatureScheme};

use crate::error::{Error, Result};
use crate::textfmt::hex;
use crate::x509;

/// How the client checks the PLC certificate.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum CertVerify {
    /// No check of the certificate (the handshake signature is still checked).
    #[default]
    None,
    /// The SHA-256 fingerprint of the PLC certificate (DER) must be one of these.
    Fingerprint(Vec<[u8; 32]>),
    /// The PLC certificate must chain to one of these certificates (the PLC
    /// certificate itself or the CA of the TIA Portal project), be valid now
    /// and be valid for the server name (IP address or DNS name).
    Trusted(Vec<CertificateDer<'static>>),
}

impl CertVerify {
    /// Parses fingerprints: 64 hex digits each, with or without ':' or
    /// spaces, separated by ',', ';' or new lines.
    pub fn fingerprints(text: &str) -> Result<CertVerify> {
        let mut out = Vec::new();
        for part in text.split([',', ';', '\n']) {
            let h: String = part.chars().filter(|c| !matches!(c, ':' | ' ' | '\t' | '\r' | '-')).collect();
            if h.is_empty() {
                continue;
            }
            let h = h.strip_prefix("sha256/").unwrap_or(&h).to_string();
            if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(Error::Certificate(format!("{:?} is not a SHA-256 fingerprint (64 hex digits)", part.trim())));
            }
            let mut f = [0u8; 32];
            for (i, b) in f.iter_mut().enumerate() {
                *b = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap_or(0);
            }
            out.push(f);
        }
        if out.is_empty() {
            return Err(Error::Certificate("no fingerprint given".into()));
        }
        Ok(CertVerify::Fingerprint(out))
    }

    /// Parses trusted certificates in PEM ("-----BEGIN CERTIFICATE-----").
    pub fn trusted_pem(pem: &str) -> Result<CertVerify> {
        let mut out = Vec::new();
        for c in CertificateDer::pem_slice_iter(pem.as_bytes()) {
            out.push(c.map_err(|e| Error::Certificate(format!("trusted certificates: {e}")))?);
        }
        if out.is_empty() {
            return Err(Error::Certificate("no PEM certificate (-----BEGIN CERTIFICATE-----) given".into()));
        }
        let mut roots = RootCertStore::empty();
        for c in &out {
            roots.add(c.clone()).map_err(|e| Error::Certificate(format!("trusted certificate: {e}")))?;
        }
        Ok(CertVerify::Trusted(out))
    }

    /// "none", "fingerprint" or "trusted".
    pub fn mode(&self) -> &'static str {
        match self {
            CertVerify::None => "none",
            CertVerify::Fingerprint(_) => "fingerprint",
            CertVerify::Trusted(_) => "trusted",
        }
    }
}

impl fmt::Debug for CertVerify {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CertVerify::None => f.write_str("None"),
            CertVerify::Fingerprint(l) => write!(f, "Fingerprint({})", l.len()),
            CertVerify::Trusted(l) => write!(f, "Trusted({})", l.len()),
        }
    }
}

/// SHA-256 of a DER certificate.
pub fn fingerprint(der: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, der);
    let mut f = [0u8; 32];
    f.copy_from_slice(d.as_ref());
    f
}

/// A fingerprint as "AB:9F:...".
pub fn fingerprint_text(f: &[u8; 32]) -> String {
    f.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// TLS options of a connection.
#[derive(Clone, Default)]
pub struct TlsOptions {
    /// None = TLS 1.3 with the ring provider and the check of `verify`.
    /// A caller configuration must do its own certificate check: with
    /// `verify` set as well, the connection fails with [`Error::Certificate`].
    pub config: Option<Arc<rustls::ClientConfig>>,
    /// Optional NSS key log (Wireshark), e.g. [`NssKeyLog`].
    pub key_log: Option<Arc<dyn rustls::KeyLog>>,
    /// Server name (SNI and the name check of [`CertVerify::Trusted`]);
    /// empty = the host part of the address (an IP address sends no SNI).
    pub server_name: String,
    /// Check of the PLC certificate (default: none). Used when `config` is None.
    pub verify: CertVerify,
}

impl fmt::Debug for TlsOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsOptions")
            .field("config", &self.config.is_some())
            .field("key_log", &self.key_log.is_some())
            .field("server_name", &self.server_name)
            .field("verify", &self.verify)
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

/// Checks the PLC certificate as [`CertVerify`] says; keeps the reason of a
/// refusal for a clear error message.
#[derive(Debug)]
pub(crate) struct PlcCertVerifier {
    provider: Arc<CryptoProvider>,
    verify: CertVerify,
    roots: RootCertStore,
    refused: Mutex<Option<(String, Vec<u8>)>>,
}

impl PlcCertVerifier {
    pub(crate) fn new(verify: CertVerify) -> Self {
        let mut roots = RootCertStore::empty();
        if let CertVerify::Trusted(l) = &verify {
            for c in l {
                let _ = roots.add(c.clone()); // checked by CertVerify::trusted_pem
            }
        }
        PlcCertVerifier { provider: provider(), verify, roots, refused: Mutex::new(None) }
    }

    /// The reason of the last refusal and the refused certificate (DER).
    pub(crate) fn refused(&self) -> Option<(String, Vec<u8>)> {
        self.refused.lock().ok()?.clone()
    }

    fn check(
        &self,
        ee: &CertificateDer<'_>,
        im: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        now: UnixTime,
    ) -> std::result::Result<(), String> {
        let fp = fingerprint(ee);
        let who = || {
            let subject = x509::parse(ee).map(|c| c.subject).unwrap_or_default();
            format!(
                "the PLC certificate ({}SHA-256 {})",
                if subject.is_empty() { String::new() } else { format!("{subject}, ") },
                fingerprint_text(&fp)
            )
        };
        match &self.verify {
            CertVerify::None => Ok(()),
            CertVerify::Fingerprint(l) => {
                if l.contains(&fp) {
                    Ok(())
                } else {
                    Err(format!("{} does not match the fingerprint in the endpoint", who()))
                }
            }
            CertVerify::Trusted(_) => {
                let parsed = ParsedCertificate::try_from(ee).map_err(|e| format!("{}: {e}", who()))?;
                verify_server_cert_signed_by_trust_anchor(
                    &parsed,
                    &self.roots,
                    im,
                    now,
                    self.provider.signature_verification_algorithms.all,
                )
                .map_err(|e| format!("{} {}", who(), cert_reason(&e)))?;
                verify_server_name(&parsed, name).map_err(|e| {
                    let n = match name {
                        ServerName::IpAddress(ip) => std::net::IpAddr::from(*ip).to_string(),
                        ServerName::DnsName(d) => d.as_ref().to_string(),
                        _ => "the server name".into(),
                    };
                    format!("{} {} (name {n}; set the certificate name in the endpoint)", who(), cert_reason(&e))
                })
            }
        }
    }
}

fn cert_reason(e: &rustls::Error) -> String {
    match e {
        rustls::Error::InvalidCertificate(c) => match c {
            CertificateError::UnknownIssuer => "is not signed by a trusted certificate".into(),
            CertificateError::BadSignature => "is not signed by a trusted certificate (bad signature)".into(),
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => "has expired".into(),
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => "is not valid yet (check the clocks)".into(),
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => "is not valid for this name".into(),
            CertificateError::InvalidPurpose | CertificateError::InvalidPurposeContext { .. } => "is not a TLS server certificate".into(),
            CertificateError::Revoked => "is revoked".into(),
            o => format!("is not valid ({o:?})"),
        },
        o => format!("is not valid ({o})"),
    }
}

impl ServerCertVerifier for PlcCertVerifier {
    fn verify_server_cert(
        &self,
        ee: &CertificateDer<'_>,
        im: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        match self.check(ee, im, name, now) {
            Ok(()) => Ok(ServerCertVerified::assertion()),
            Err(why) => {
                if let Ok(mut r) = self.refused.lock() {
                    *r = Some((why, ee.to_vec()));
                }
                Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Builds the client configuration of a connection; also returns the
/// certificate verifier (None for a caller `config`).
pub(crate) fn client_config(o: &TlsOptions) -> Result<(Arc<rustls::ClientConfig>, Option<Arc<PlcCertVerifier>>)> {
    let (mut cfg, ver) = match &o.config {
        // a caller configuration has its own verifier: a check here would be ignored
        Some(_) if o.verify != CertVerify::None => {
            return Err(Error::Certificate("TlsOptions: set either config (with its own certificate verifier) or verify, not both".into()))
        }
        Some(c) => ((**c).clone(), None),
        None => {
            let v = Arc::new(PlcCertVerifier::new(o.verify.clone()));
            let cfg = rustls::ClientConfig::builder_with_provider(v.provider.clone())
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| Error::Tls(e.to_string()))?
                .dangerous()
                .with_custom_certificate_verifier(v.clone())
                .with_no_client_auth();
            (cfg, Some(v))
        }
    };
    if let Some(kl) = &o.key_log {
        cfg.key_log = kl.clone();
    }
    Ok((Arc::new(cfg), ver))
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
