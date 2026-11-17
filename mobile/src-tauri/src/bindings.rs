//! Types TypeScript propres à l'adaptation mobile (`mobile/src/lib/bindings/mobile.ts`), générés
//! avec l'assembleur du core (`minestrator_core::bindings`). Les types du core, eux, vont dans
//! `mobile/src/lib/bindings/core.ts` (produit par le core). Régénérer : `cargo bindings`.

use minestrator_core::bindings::{self, Bindings};

/// Fichier alimenté par ce crate, relatif à la racine du dépôt.
const TARGET: &str = "mobile/src/lib/bindings/mobile.ts";

fn mobile() -> Bindings {
    Bindings::new("mobile/src-tauri, feature `ts`")
        .import("./core", &["SftpEntry"])
        .section("Mise à jour de l'APK (update.rs)")
        .add::<crate::update::UpdateInfo>()
        .section("SFTP (commands.rs)")
        .add::<crate::commands::SftpSearchOut>()
}

#[test]
fn bindings_are_self_contained() {
    assert_eq!(mobile().missing(), Vec::<String>::new(), "types référencés mais non déclarés");
}

#[test]
fn bindings_are_up_to_date() {
    let root = bindings::repo_root(env!("CARGO_MANIFEST_DIR"));
    if let Err(e) = bindings::check(&root, TARGET, &mobile().render()) {
        panic!("{e}");
    }
}

#[test]
#[ignore = "réécrit le fichier versionné : lancé par `cargo bindings`"]
fn regenerate_bindings() {
    let root = bindings::repo_root(env!("CARGO_MANIFEST_DIR"));
    bindings::write(&root, TARGET, &mobile().render()).expect(TARGET);
}
