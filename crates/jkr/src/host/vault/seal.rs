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

use tkr_api::vault::SealState;
use zeroize::Zeroize;

pub struct SealStateMachine {
    master: [u8; 32],
    pub_key: Option<[u8; 32]>,
    priv_key: Option<[u8; 32]>,
    sec_key: Option<[u8; 32]>,
}

impl SealStateMachine {
    pub fn sealed(master: [u8; 32]) -> Self {
        Self {
            master,
            pub_key: None,
            priv_key: None,
            sec_key: None,
        }
    }

    pub fn auto_unseal(master: [u8; 32]) -> Self {
        let mut s = Self::sealed(master);
        s.pub_key = Some(derive_subkey(&s.master, SensitivityClass::Public));
        s
    }

    pub fn full_unseal(&mut self) {
        if self.pub_key.is_none() {
            self.pub_key = Some(derive_subkey(&self.master, SensitivityClass::Public));
        }
        if self.priv_key.is_none() {
            self.priv_key = Some(derive_subkey(&self.master, SensitivityClass::Private));
        }
        if self.sec_key.is_none() {
            self.sec_key = Some(derive_subkey(&self.master, SensitivityClass::Secret));
        }
    }

    pub fn seal(&mut self) {
        if let Some(mut k) = self.pub_key.take() {
            k.zeroize();
        }
        if let Some(mut k) = self.priv_key.take() {
            k.zeroize();
        }
        if let Some(mut k) = self.sec_key.take() {
            k.zeroize();
        }
    }

    pub fn state(&self) -> SealState {
        match (
            self.pub_key.is_some(),
            self.priv_key.is_some() && self.sec_key.is_some(),
        ) {
            (false, _) => SealState::Sealed,
            (true, false) => SealState::AutoUnsealed,
            (true, true) => SealState::FullyUnsealed,
        }
    }

    pub fn subkey(&self, class: SensitivityClass) -> Option<&[u8; 32]> {
        match class {
            SensitivityClass::Public => self.pub_key.as_ref(),
            SensitivityClass::Private => self.priv_key.as_ref(),
            SensitivityClass::Secret => self.sec_key.as_ref(),
        }
    }
}

impl Drop for SealStateMachine {
    fn drop(&mut self) {
        self.master.zeroize();
        self.seal();
    }
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

#[cfg(test)]
mod tests_state {
    use super::*;

    #[test]
    fn auto_unseal_grants_public_only() {
        let m = SealStateMachine::auto_unseal([0u8; 32]);
        assert_eq!(m.state(), SealState::AutoUnsealed);
        assert!(m.subkey(SensitivityClass::Public).is_some());
        assert!(m.subkey(SensitivityClass::Private).is_none());
        assert!(m.subkey(SensitivityClass::Secret).is_none());
    }

    #[test]
    fn full_unseal_grants_all() {
        let mut m = SealStateMachine::auto_unseal([0u8; 32]);
        m.full_unseal();
        assert_eq!(m.state(), SealState::FullyUnsealed);
        assert!(m.subkey(SensitivityClass::Secret).is_some());
    }

    #[test]
    fn seal_zeroes_subkeys() {
        let mut m = SealStateMachine::auto_unseal([0u8; 32]);
        m.full_unseal();
        m.seal();
        assert_eq!(m.state(), SealState::Sealed);
        assert!(m.subkey(SensitivityClass::Public).is_none());
    }
}
