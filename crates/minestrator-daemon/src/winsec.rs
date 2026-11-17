#![cfg_attr(not(windows), allow(dead_code))]
//! Descripteurs de sécurité Windows : DACL du canal nommé IPC et du dossier de données.
//!
//! Les chaînes SDDL sont construites ici (fonctions pures, testées sur toutes les plateformes) ;
//! leur application n'existe que sous Windows.
//!
//! - **Dossier de données** : `SYSTEM`, `Administrateurs` et `LocalService` (le compte du service)
//!   seulement, héritage parent coupé. Par défaut, un sous-dossier de `%ProgramData%` est lisible
//!   par tous les utilisateurs : sans cette DACL, `secrets.json` le serait aussi.
//! - **Canal nommé** : `SYSTEM` et `Administrateurs` (accès complet) + chaque propriétaire
//!   autorisé, en lecture/écriture. `FILE_GENERIC_WRITE` inclut `FILE_CREATE_PIPE_INSTANCE`
//!   (même bit que `FILE_APPEND_DATA`) : on ne peut pas le retirer, car un client ouvre le canal
//!   en `GENERIC_WRITE`. Un propriétaire pourrait donc ouvrir une instance serveur concurrente ;
//!   l'app s'en protège en vérifiant que le PID du serveur est celui du service (SCM).

/// DACL du dossier de données (protégée, héritée par les fichiers et sous-dossiers).
pub const DATA_DIR_SDDL: &str = "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)";

/// Droits d'un client du canal : `FILE_GENERIC_READ | FILE_GENERIC_WRITE`.
pub const PIPE_CLIENT_RIGHTS: &str = "0x12019f";

/// DACL du canal nommé pour une liste de SID autorisés (déjà validés par l'appelant).
pub fn pipe_sddl<'a>(owners: impl IntoIterator<Item = &'a str>) -> String {
    let mut sddl = String::from("D:P(A;;GA;;;SY)(A;;GA;;;BA)");
    for sid in owners {
        if minestrator_core::service::is_valid_sid(sid) {
            sddl.push_str(&format!("(A;;{PIPE_CLIENT_RIGHTS};;;{sid})"));
        }
    }
    sddl
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SDDL_REVISION_1,
        SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    };

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Descripteur de sécurité alloué par Windows depuis une chaîne SDDL (libéré au drop).
    pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

    // Le descripteur est immuable après création : le partager entre threads est sûr.
    unsafe impl Send for SecurityDescriptor {}
    unsafe impl Sync for SecurityDescriptor {}

    impl SecurityDescriptor {
        pub fn from_sddl(sddl: &str) -> io::Result<Self> {
            let w = wide(std::ffi::OsStr::new(sddl));
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY : chaîne terminée par un nul ; `sd` reçoit un bloc LocalAlloc libéré au drop.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    w.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(sd))
        }

        fn dacl(&self) -> io::Result<*mut ACL> {
            let mut present = 0;
            let mut defaulted = 0;
            let mut dacl: *mut ACL = std::ptr::null_mut();
            // SAFETY : descripteur valide ; la DACL pointe dans le descripteur (même durée de vie).
            let ok = unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) };
            if ok == 0 || present == 0 {
                return Err(io::Error::other("DACL absente du descripteur"));
            }
            Ok(dacl)
        }
    }

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY : bloc alloué par ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe {
                LocalFree(self.0);
            }
        }
    }

    /// Attributs de sécurité d'un canal nommé (à garder vivants tant que des instances sont créées).
    pub struct PipeSecurity {
        _sd: SecurityDescriptor,
        attrs: Box<SECURITY_ATTRIBUTES>,
    }

    unsafe impl Send for PipeSecurity {}
    unsafe impl Sync for PipeSecurity {}

    impl PipeSecurity {
        pub fn new(sddl: &str) -> io::Result<Self> {
            let sd = SecurityDescriptor::from_sddl(sddl)?;
            let attrs = Box::new(SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: 0,
            });
            Ok(Self { _sd: sd, attrs })
        }

        /// Pointeur pour `ServerOptions::create_with_security_attributes_raw`.
        pub fn as_ptr(&self) -> *mut c_void {
            (&*self.attrs as *const SECURITY_ATTRIBUTES).cast_mut().cast()
        }
    }

    /// Applique [`super::DATA_DIR_SDDL`] au dossier (et, par héritage, à son contenu).
    pub fn secure_dir(dir: &Path) -> io::Result<()> {
        let sd = SecurityDescriptor::from_sddl(super::DATA_DIR_SDDL)?;
        let dacl = sd.dacl()?;
        let path = wide(dir.as_os_str());
        // SAFETY : chemin terminé par un nul ; DACL valide pendant l'appel (portée de `sd`).
        let rc = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sddl_du_canal() {
        let sddl = pipe_sddl(["S-1-5-21-1-2-3-1001", "S-1-5-19"]);
        assert_eq!(
            sddl,
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019f;;;S-1-5-21-1-2-3-1001)(A;;0x12019f;;;S-1-5-19)"
        );
        // Une tentative d'injection d'ACE est ignorée (SID invalide).
        let evil = pipe_sddl(["S-1-5-21-1)(A;;GA;;;WD"]);
        assert_eq!(evil, "D:P(A;;GA;;;SY)(A;;GA;;;BA)");
    }

    #[test]
    fn sddl_du_dossier_sans_utilisateurs() {
        // Ni Users (BU), ni Everyone (WD), ni Authenticated Users (AU).
        for forbidden in [";BU)", ";WD)", ";AU)"] {
            assert!(!DATA_DIR_SDDL.contains(forbidden));
        }
        assert!(DATA_DIR_SDDL.starts_with("D:P"));
    }

    #[cfg(windows)]
    #[test]
    fn sddl_accepte_par_windows() {
        assert!(SecurityDescriptor::from_sddl(DATA_DIR_SDDL).is_ok());
        assert!(PipeSecurity::new(&pipe_sddl(["S-1-5-21-1-2-3-1001"])).is_ok());
    }

    /// Un compte absent de la DACL du canal est refusé, et le client le signale comme tel
    /// (`forbidden`, pas `unreachable`).
    #[cfg(windows)]
    #[tokio::test]
    async fn canal_refuse_aux_comptes_non_autorises() {
        use minestrator_core::service::{request, Request};
        use minestrator_core::Error;
        use tokio::net::windows::named_pipe::ServerOptions;
        let name = format!(r"\\.\pipe\minestrator-daemon-test-acl-{}", std::process::id());
        // SYSTEM seulement : le compte qui lance les tests n'y figure pas.
        let security = PipeSecurity::new("D:P(A;;GA;;;SY)").unwrap();
        // SAFETY : `security` vit jusqu'à la fin du test.
        let _server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .create_with_security_attributes_raw(&name, security.as_ptr())
                .unwrap()
        };
        std::env::set_var(minestrator_core::service::ENDPOINT_ENV, &name);
        let err = request(&name, &Request::Ping, std::time::Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Service { ref code, .. } if code == "forbidden"), "{err:?}");
    }
}
