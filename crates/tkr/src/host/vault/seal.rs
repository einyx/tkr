use hkdf::Hkdf;
use sha2::Sha256;
use tkr_api::manifest::SensitivityClass;

pub fn derive_subkey(master: &[u8; 32], class: SensitivityClass) -> [u8; 32] {
    let info: &[u8] = match class {
        SensitivityClass::Public => b"tkr.vault.subkey.public",
        SensitivityClass::Private => b"tkr.vault.subkey.private",
        SensitivityClass::Secret => b"tkr.vault.subkey.secret",
    };
    let hk = Hkdf::<Sha256>::new(None, master);
    let mut out = [0u8; 32];
    hk.expand(info, &mut out).expect("32 bytes <= 255*HashLen");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subkeys_differ_per_class() {
        let m = [1u8; 32];
        let a = derive_subkey(&m, SensitivityClass::Public);
        let b = derive_subkey(&m, SensitivityClass::Private);
        let c = derive_subkey(&m, SensitivityClass::Secret);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn deterministic() {
        let a = derive_subkey(&[2u8; 32], SensitivityClass::Public);
        let b = derive_subkey(&[2u8; 32], SensitivityClass::Public);
        assert_eq!(a, b);
    }
}
