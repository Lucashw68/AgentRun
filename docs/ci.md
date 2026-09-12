# CI et binaires Linux

Le workflow [CI](../.github/workflows/ci.yml) s'exécute sur les push, pull requests et déclenchements manuels (`Actions → CI → Run workflow`). Il utilise deux machines GitHub natives : Ubuntu 24.04 x86_64 et Ubuntu 24.04 ARM64.

Chaque architecture exécute les mêmes étapes :

1. Vérification de Linux 6.9 minimum, nécessaire aux signaux de groupe via pidfd. Les tests échouent si les appels système requis sont interdits ; ils ne sont pas ignorés.
2. Installation de Rust 1.98.1 avec rustfmt et Clippy.
3. Formatage, Clippy avec avertissements traités comme erreurs, puis tous les tests Rust, dont les intégrations avec de vrais processus Linux.
4. Compilation release des trois binaires statiques musl, avec `rust-lld`, `crt-static` et dépendances verrouillées par `Cargo.lock`. Contrôle ELF de l'architecture, de l'absence d'interpréteur dynamique et de bibliothèques partagées requises.
5. Dix cycles MCP start/logs/restart/stop/clean sur les binaires release, avec contrôle des ressources et de l'absence de sockets du MCP.
6. Création des archives et sommes SHA-256, tests de l'installateur hors ligne, puis installation et essais CLI/MCP dans Ubuntu, Fedora, Debian et Alpine. Arch est testé sur x86_64. Les conteneurs tournent sans réseau, sans capacités Linux et avec un utilisateur non privilégié ; ils partagent le noyau du runner.
7. Tests du téléchargement automatique avec un transport substitué et de vraies archives : modes public/privé, version choisie, échecs réseau, checksums incorrects, membres manquants, dupliqués ou symlinks, et conservation d'une installation existante. Les téléchargements GitHub réels sont vérifiés séparément avant publication.
8. Dépôt des artifacts après succès de toutes les vérifications, dont `install-agentrun.sh` et son checksum.

Les [images x86_64](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md) et [ARM64](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Arm64-Readme.md) documentent leur noyau courant. La vérification du workflow protège contre un changement d'image incompatible. Le runner ARM64 doit être disponible pour le dépôt et son offre GitHub.

## Récupérer une compilation

Dans `Actions`, ouvrir une exécution réussie, puis télécharger l'artifact correspondant :

- `agentrun-x86_64-unknown-linux-musl`
- `agentrun-aarch64-unknown-linux-musl`

Après extraction de l'artifact GitHub, on obtient une archive `agentrun-<version>-<target>.tar.gz` et son fichier `.tar.gz.sha256`. Vérifier l'archive avec `sha256sum -c <archive>.sha256`, puis l'extraire. Elle contient `agentrun`, `agentrun-mcp`, `agentrun-log`, le README, la licence, la configuration initiale et les guides. Installer **les trois exécutables dans le même répertoire**. L'archive tar conserve leurs permissions exécutables.

Les artifacts de CI sont conservés 14 jours. Les binaires statiques sont indépendants de la glibc de la distribution ; Linux 6.9+ reste nécessaire à l'exécution. L'installateur `install.sh` est inclus. Voir le [guide d'installation](install.md) pour les limites de compatibilité et les commandes.

Les versions publiées sont aussi disponibles dans l'onglet **Releases**, avec les mêmes archives validées et leurs checksums. La publication d'une release est une étape distincte ; le workflow CI ne publie pas automatiquement de release ni de paquet sur crates.io.

## Permissions et maintenance

Le token GitHub possède seulement `contents: read`. Les actions officielles checkout et upload-artifact sont épinglées par SHA, les identifiants Git ne sont pas conservés dans le checkout, et aucun secret de dépôt n'est utilisé. Les pull requests tournent sur des runners GitHub éphémères via `pull_request`. Les versions de Rust et les SHA des actions sont à mettre à jour explicitement.

Les chemins du checkout sont remappés dans les builds ; les archives ne contiennent pas les noms d'utilisateurs ni les chemins d'origine dans leurs métadonnées. Les checksums détectent une corruption, ils ne sont pas une signature de provenance. Préférer les artifacts issus de la branche de confiance ; une pull request peut modifier le code compilé.

## Reproduire localement

Depuis le dépôt, avec Rust et Python 3.11+ :

```bash
cargo fmt --all -- --check
rustup target add x86_64-unknown-linux-musl
export RUSTFLAGS='-C linker=rust-lld -C target-feature=+crt-static'
cargo clippy --locked --all-targets --target x86_64-unknown-linux-musl -- -D warnings
cargo test --locked --all-targets --target x86_64-unknown-linux-musl
cargo build --locked --release --bins --target x86_64-unknown-linux-musl
python3 scripts/verify_static.py --bin-dir target/x86_64-unknown-linux-musl/release --target x86_64-unknown-linux-musl
python3 scripts/resource_soak.py --bin-dir target/x86_64-unknown-linux-musl/release --cycles 10
python3 scripts/package_binaries.py --bin-dir target/x86_64-unknown-linux-musl/release --target x86_64-unknown-linux-musl
python3 scripts/test_install.py --archive dist/agentrun-0.3.2-x86_64-unknown-linux-musl.tar.gz
python3 scripts/test_download_install.py --archive dist/agentrun-0.3.2-x86_64-unknown-linux-musl.tar.gz
python3 scripts/distribution_smoke.py --archive dist/agentrun-0.3.2-x86_64-unknown-linux-musl.tar.gz
```

Pour ARM64, utiliser la cible `aarch64-unknown-linux-musl` sur une machine ARM64. Le script d'archive vérifie les binaires mais ne réalise aucune compilation croisée. Le dernier test nécessite Docker et télécharge les images officielles des distributions ; Docker n'est pas requis pour installer ou utiliser AgentRun.
