# CI et binaires Linux

Le workflow [CI](../.github/workflows/ci.yml) s'exécute sur les push, pull requests et déclenchements manuels (`Actions → CI → Run workflow`). Il utilise deux machines GitHub natives : Ubuntu 24.04 x86_64 et Ubuntu 24.04 ARM64.

Chaque architecture exécute les mêmes étapes :

1. Vérification de Linux 6.9 minimum, nécessaire aux signaux de groupe via pidfd. Les tests échouent si les appels système requis sont interdits ; ils ne sont pas ignorés.
2. Installation de Rust 1.98.1 avec rustfmt et Clippy.
3. Formatage, Clippy avec avertissements traités comme erreurs, puis tous les tests Rust, dont les intégrations avec de vrais processus Linux.
4. Compilation release des trois binaires, avec dépendances verrouillées par `Cargo.lock`.
5. Dix cycles MCP start/logs/restart/stop/clean sur les binaires release, avec contrôle des ressources et de l'absence de sockets du MCP.
6. Création et dépôt des archives et sommes SHA-256, après succès des étapes précédentes.

Les [images x86_64](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md) et [ARM64](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Arm64-Readme.md) documentent leur noyau courant. La vérification du workflow protège contre un changement d'image incompatible. Le runner ARM64 doit être disponible pour le dépôt et son offre GitHub.

## Récupérer une compilation

Dans `Actions`, ouvrir une exécution réussie, puis télécharger l'artifact correspondant :

- `agentrun-x86_64-unknown-linux-gnu`
- `agentrun-aarch64-unknown-linux-gnu`

Après extraction de l'artifact GitHub, on obtient une archive `agentrun-<version>-<target>.tar.gz` et son fichier `.tar.gz.sha256`. Vérifier l'archive avec `sha256sum -c <archive>.sha256`, puis l'extraire. Elle contient `agentrun`, `agentrun-mcp`, `agentrun-log`, le README, la licence, la configuration initiale et les guides. Installer **les trois exécutables dans le même répertoire**. L'archive tar conserve leurs permissions exécutables.

Les fichiers sont conservés 14 jours. Ils ciblent Linux/glibc, avec les bibliothèques d'Ubuntu 24.04 comme environnement de compilation ; pour une distribution plus ancienne ou musl, compiler localement. Linux 6.9+ reste nécessaire à l'exécution. Ce workflow ne publie pas de GitHub Release ni de paquet sur crates.io.

## Permissions et maintenance

Le token GitHub possède seulement `contents: read`. Les actions officielles checkout et upload-artifact sont épinglées par SHA, les identifiants Git ne sont pas conservés dans le checkout, et aucun secret de dépôt n'est utilisé. Les pull requests tournent sur des runners GitHub éphémères via `pull_request`. Les versions de Rust et les SHA des actions sont à mettre à jour explicitement.

Les chemins du checkout sont remappés dans les builds ; les archives ne contiennent pas les noms d'utilisateurs ni les chemins d'origine dans leurs métadonnées. Les checksums détectent une corruption, ils ne sont pas une signature de provenance. Préférer les artifacts issus de la branche de confiance ; une pull request peut modifier le code compilé.

## Reproduire localement

Depuis le dépôt, avec Rust et Python 3.11+ :

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release --bins
python3 scripts/resource_soak.py --bin-dir target/release --cycles 10
python3 scripts/package_binaries.py --bin-dir target/release --target x86_64-unknown-linux-gnu
```

Pour ARM64, utiliser la cible `aarch64-unknown-linux-gnu` sur une machine ARM64. Le paramètre du script d'archive nomme la plateforme ; il ne réalise aucune compilation croisée. Le workflow sera effectivement exécuté après l'envoi des fichiers dans le dépôt GitHub.
