---
name: release-changelog
description: Rédige l'entrée de CHANGELOG.md d'une nouvelle version de Minestrator Terminal, puis publie la release (bump des 5 fichiers de version, commits, tag, suivi de la CI). Utiliser dès qu'on prépare une version, qu'on parle de "faire la 0.x.y", "publier une release", "préparer le changelog", ou qu'on veut réécrire des notes de version existantes.
---

# Notes de version — Minestrator Terminal

`CHANGELOG.md` (racine) est la **source unique** des notes de version : la CI en extrait la section
de la version taguée pour remplir le corps de la release GitHub. Une version sans entrée dans ce
fichier sort avec des notes vides.

## Écrire pour l'utilisateur, pas pour le dépôt

Le lecteur installe un client de gestion de serveurs de jeu. Il veut savoir **ce qui change pour
lui**, pas comment c'est fait.

- Décrire l'effet visible : « les envois interrompus reprennent où ils s'étaient arrêtés », pas
  « ajout de `upload_stream_from` avec offset ».
- Bannir les noms de fonctions, de fichiers, de crates et de commits. Aucun `with_sftp`, aucun
  `emit_transfer_end`, aucun hash.
- Une ligne = un changement perceptible. Si un changement ne se voit ni ne se sent, il n'a pas sa
  place ici (refactors internes, migrations mécaniques, nettoyage de code mort).
- Français correct, accents compris. Phrases complètes, ton neutre. Ni superlatifs, ni emojis, ni
  « 🎉 nouveauté incroyable ».
- Quand un correctif répare quelque chose que l'utilisateur a pu subir, le dire franchement :
  « la première action après une veille du téléphone échouait systématiquement ». C'est plus utile
  qu'un « améliorations de stabilité » qui n'apprend rien.

## Structure d'une entrée

```markdown
## v0.5.2

### <Thème>

- Changement décrit par son effet.

### Corrections

- Ce qui ne marchait pas et marche désormais.
```

- Titre exact : `## v<major>.<minor>.<patch>`, sans date (elle figure sur la release GitHub).
- Regrouper par **thème utile au lecteur** — « Application mobile », « Fiabilité des transferts »,
  « Confidentialité » — plutôt que par couche technique (core / desktop / mobile) sauf quand la
  distinction change ce que le lecteur doit faire.
- Terminer par `### Corrections` s'il y en a.
- Le plus marquant en premier, dans chaque section comme entre sections.
- Séparer les versions par `---`.

## Préparer une version

1. **Rédiger l'entrée** en tête de `CHANGELOG.md`, à partir des commits depuis le dernier tag
   (`git log --oneline <dernier-tag>..HEAD`) **et** de ce qui a réellement été fait dans la
   session — les messages de commit sont un aide-mémoire, pas la source.
2. **Bump des 5 fichiers**, tous à la même valeur :
   `Cargo.toml` (racine, `[workspace.package]`), `desktop/package.json`, `mobile/package.json`,
   `desktop/src-tauri/tauri.conf.json`, `mobile/src-tauri/tauri.conf.json`.
   Puis `cargo check --workspace` pour régénérer `Cargo.lock`.
3. **Valider** : `cargo test -p minestrator-core`, `cargo clippy --workspace --all-targets`,
   `npm run check` dans `desktop/` **et** `mobile/`.
4. **Deux commits** distincts, dans cet ordre : le contenu fonctionnel, puis
   `chore(release): v<version>` qui ne porte **que** le bump et le changelog.
5. `git push origin main` puis `git tag v<version> && git push origin v<version>`.
6. La CI construit les installeurs des trois OS et l'APK Android signé, puis crée une release
   **draft**. La publier à la main : l'endpoint d'auto-update ne résout que vers une release
   publiée, donc rien n'est proposé aux clients tant qu'elle reste en draft.

## Choisir le numéro

- **Patch** (`0.5.1` → `0.5.2`) : corrections, ajustements, rien de neuf à découvrir.
- **Mineure** (`0.5.2` → `0.6.0`) : nouvelles fonctionnalités visibles.
- Vérifier les tags existants avant de choisir (`git tag --sort=-v:refname | head`) : une version
  déjà publiée ne se réutilise pas.

## Pièges déjà rencontrés

- Les `package.json` sont les plus souvent oubliés au bump (c'est arrivé en 0.4.2, restés à 0.4.1).
- Le tag déclenche la CI : ne le pousser qu'après le commit de release, sinon la version construite
  ne correspond pas au tag.
- La CI a déjà calé sur un `apt-get` figé côté runner GitHub. Avant de toucher au workflow,
  relancer le job — deux échecs d'affilée au même endroit signalent un vrai problème, un seul non.
