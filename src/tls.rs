use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use rustls::ServerConfig as TlsServerConfig;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

use crate::config::Config;

pub fn build_tls_acceptor(config: &Config) -> anyhow::Result<Option<TlsAcceptor>> {
    if let (Some(cert_path), Some(key_path)) = (&config.tls.cert_path, &config.tls.key_path) {
        if cert_path.exists() && key_path.exists() {
            log::info!("Loading TLS cert from {:?} and key from {:?}", cert_path, key_path);

            let certs = CertificateDer::pem_file_iter(cert_path)
                .map_err(|e| anyhow::anyhow!("Failed to read cert file: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| anyhow::anyhow!("Failed to parse cert: {}", e))?;

            let key = PrivateKeyDer::from_pem_file(key_path)
                .map_err(|e| anyhow::anyhow!("Failed to read key file: {}", e))?;

            let tls_config = TlsServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|e| anyhow::anyhow!("TLS config error: {}", e))?;

            return Ok(Some(TlsAcceptor::from(Arc::new(tls_config))));
        }
    }

    if config.tls.generate_self_signed {
        log::warn!("Generating self-signed certificate for {}", config.server.domain);
        let cert = generate_self_signed_cert(&config.server.domain, config.tls.self_signed_days)?;
        return Ok(Some(cert));
    }

    log::warn!("No TLS configured, running without encryption");
    Ok(None)
}

fn generate_self_signed_cert(domain: &str, _days: u32) -> anyhow::Result<TlsAcceptor> {
    use rcgen::{CertificateParams, KeyPair, DistinguishedName, IsCa, BasicConstraints, ExtendedKeyUsagePurpose, KeyUsagePurpose};

    let mut params = CertificateParams::new(vec![domain.to_string()])
        .map_err(|e| anyhow::anyhow!("Failed to create cert params: {}", e))?;

    params.distinguished_name = DistinguishedName::new();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
    ];

    let key_pair = KeyPair::generate()
        .map_err(|e| anyhow::anyhow!("Failed to generate key pair: {}", e))?;

    let cert = params.self_signed(&key_pair)
        .map_err(|e| anyhow::anyhow!("Failed to self-sign cert: {}", e))?;

    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();

    let cert = CertificateDer::from(cert_der);
    let key = PrivateKeyDer::try_from(key_der)
        .map_err(|e| anyhow::anyhow!("Failed to create private key: {}", e))?;

    let tls_config = TlsServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| anyhow::anyhow!("TLS config error: {}", e))?;

    Ok(TlsAcceptor::from(Arc::new(tls_config)))
}
