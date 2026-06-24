use rcgen::{CertificateParams, DistinguishedName, DnType};

fn main() {
    let mut params = CertificateParams::new(vec!["localhost".to_string()]);
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "Messenger Dev");

    let cert = rcgen::Certificate::from_params(params).expect("cert generation failed");
    let cert_pem = cert.serialize_pem().expect("serialize cert");
    let key_pem = cert.serialize_private_key_pem();

    std::fs::write("cert.pem", &cert_pem).expect("write cert.pem");
    std::fs::write("key.pem", &key_pem).expect("write key.pem");

    println!("cert.pem and key.pem written to current directory");
    println!("To use on clients: set MESSENGER_INSECURE_TLS=1 for self-signed certs");
    println!("To add to Windows trust store: certutil -addstore Root cert.pem");
}
