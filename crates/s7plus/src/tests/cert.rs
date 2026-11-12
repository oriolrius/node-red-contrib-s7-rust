//! PLC certificate: the X.509 reader and the trusted-certificate check
//! with the certificate of PLCSIM Advanced V8 (S7-1500, FW S4.1).

use rustls::client::{verify_server_cert_signed_by_trust_anchor, verify_server_name};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;

use crate::tls::{fingerprint, fingerprint_text, provider, CertVerify};
use crate::x509;

const PLCSIM_PEM: &str = include_str!("plcsim_cert.pem");
const PLCSIM_SHA256: &str = "AB:9F:2B:B4:E3:97:2B:93:D3:DB:D3:57:65:AA:8B:79:78:34:9E:35:B8:BA:F0:FA:A0:4B:CA:7D:E5:BB:47:0C";

fn plcsim_der() -> Vec<u8> {
    match CertVerify::trusted_pem(PLCSIM_PEM).unwrap() {
        CertVerify::Trusted(l) => l[0].to_vec(),
        _ => unreachable!(),
    }
}

#[test]
fn x509_fields_of_the_plcsim_certificate() {
    let der = plcsim_der();
    let i = x509::parse(&der).expect("parse");
    assert_eq!(i.subject, "CN=PLC-1/Communication-1, OU=Industrial Communication, O=Siemens, C=DE");
    assert_eq!(i.issuer, i.subject);
    assert!(i.self_signed && !i.ca);
    assert_eq!(i.not_before, "2026-11-11T18:59:11Z");
    assert_eq!(i.not_after, "2037-11-10T23:00:00Z");
    assert_eq!(i.alt_names, vec!["IP:192.168.0.1".to_string()]);
    assert_eq!(i.fingerprint, PLCSIM_SHA256);
    assert_eq!(fingerprint_text(&fingerprint(&der)), PLCSIM_SHA256);
    // PEM round trip
    assert_eq!(x509::pem(&der).trim(), PLCSIM_PEM.trim());
    assert!(x509::parse(&der[..der.len() - 10]).is_none() || x509::parse(b"\x30\x03\x02\x01\x00").is_none());
    assert!(x509::parse(b"").is_none() && x509::parse(b"\x30\x82\xff").is_none());
}

#[test]
fn fingerprint_parsing() {
    let lower = PLCSIM_SHA256.replace(':', "").to_lowercase();
    for t in [PLCSIM_SHA256.to_string(), lower.clone(), format!("sha256/{lower}"), format!(" {PLCSIM_SHA256} ,\n{}", "00".repeat(32))] {
        match CertVerify::fingerprints(&t) {
            Ok(CertVerify::Fingerprint(l)) => assert_eq!(l[0], fingerprint(&plcsim_der()), "{t}"),
            o => panic!("{t}: {o:?}"),
        }
    }
    for bad in ["", " , ", "AB:9F", &"zz".repeat(32)] {
        let ok = CertVerify::fingerprints(bad).is_err_and(|e| e.to_string().contains("fingerprint"));
        assert!(ok, "no fingerprint error for {bad:?}");
    }
    assert!(CertVerify::trusted_pem("not a pem").unwrap_err().to_string().contains("PEM"));
}

/// The S7-1500 certificate is self-signed with CA:FALSE: webpki must accept
/// it as its own trust anchor.
#[test]
fn plcsim_certificate_trusted_by_itself() {
    let der = CertificateDer::from(plcsim_der());
    let parsed = ParsedCertificate::try_from(&der).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(der.clone()).unwrap();
    let algs = provider().signature_verification_algorithms.all;
    let now = UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_800_000_000)); // 2027-01
    verify_server_cert_signed_by_trust_anchor(&parsed, &roots, &[], now, algs).expect("self-signed PLC certificate");
    verify_server_name(&parsed, &ServerName::try_from("192.168.0.1").unwrap()).expect("IP SAN");
    assert!(verify_server_name(&parsed, &ServerName::try_from("192.168.0.2").unwrap()).is_err());
    let late = UnixTime::since_unix_epoch(std::time::Duration::from_secs(2_200_000_000)); // 2039
    assert!(verify_server_cert_signed_by_trust_anchor(&parsed, &roots, &[], late, algs).is_err());
}

#[test]
fn validity_in_unix_seconds() {
    let (from, until) = x509::validity(&plcsim_der()).unwrap();
    assert_eq!(from, 1_794_423_551); // 2026-11-11T18:59:11Z
    assert_eq!(until, 2_141_506_800); // 2037-11-10T23:00:00Z
}
