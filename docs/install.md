# Installer AgentRun sur Linux

Les releases à partir de **0.3.1** fournissent des binaires statiques musl pour **x86_64** et **ARM64**. La même archive fonctionne sur les distributions utilisant glibc ou musl : aucun compilateur Rust, runtime supplémentaire ou paquet de compatibilité glibc n'est nécessaire pour AgentRun. Les programmes lancés par ses profils ont leurs propres dépendances.

## Téléchargement et installation automatiques

**Installer la dernière release publique en une commande, sans compte GitHub ni sudo :**

```bash
curl -qfsSL --proto '=https' --proto-redir '=https' https://github.com/Lucashw68/AgentRun/releases/latest/download/install-agentrun.sh -o install-agentrun.sh && sh install-agentrun.sh --repo Lucashw68/AgentRun
```

Le script détecte l'architecture, choisit la dernière release stable, télécharge l'archive et son checksum, vérifie SHA-256 puis installe les trois binaires et l'utilitaire `agentrun-setup` dans `~/.local/bin`. Le `&&` empêche l'exécution du script si le téléchargement échoue. Pour le lire avant exécution, lancer séparément les deux commandes situées de part et d'autre du `&&`.

Ajouter le répertoire au PATH si nécessaire et vérifier l'installation :

```bash
export PATH="$HOME/.local/bin:$PATH"
agentrun --version
agentrun list --json
```

Cette ligne `export` s'applique au terminal actuel. Si `~/.local/bin` ne figure pas déjà dans le PATH, ajouter la ligne à la configuration de son shell pour les futurs terminaux, par exemple `~/.bashrc` avec Bash ou `~/.zshrc` avec Zsh. L'installateur ne modifie pas ces fichiers.

Le script exige Linux 6.9+, `curl`, `tar`, `sha256sum` et les outils POSIX usuels. Il ne télécharge aucun de ces prérequis. Les architectures prises en charge sont x86_64 et ARM64.

Pour terminer la configuration Codex :

```bash
agentrun-setup codex
```

Cet utilitaire est inclus à partir de 0.3.3 et nécessite Python 3.8+ ainsi que le CLI Codex. Il crée la politique initiale si elle manque, enregistre le MCP et ajoute les consignes permanentes, en conservant les réglages existants. Pour un autre client, utiliser `agentrun-setup config` puis les [instructions du client](agents.md). Python n'est nécessaire qu'à cet utilitaire ; le CLI et le MCP restent des binaires autonomes. La configuration automatique est une étape explicite après l'installation.

Pour choisir une version et un emplacement, réutiliser le script téléchargé :

```bash
sh install-agentrun.sh --repo Lucashw68/AgentRun \
  --version 0.3.3 --prefix "$HOME/Applications/agentrun"
export PATH="$HOME/Applications/agentrun/bin:$PATH"
```

`--version` accepte `0.3.3` ou `v0.3.3`. Sans cette option, la dernière version est résolue une fois avant les téléchargements. Pour mettre à jour, relancer la commande d'installation avec le même préfixe ; aucun suivi automatique des mises à jour n'est ajouté au CLI/MCP. Reconnecter les clients MCP pour charger les nouveaux binaires. La configuration et le registre existants sont conservés.

Les téléchargements publics et leurs redirections sont limités à HTTPS avec validation TLS ; le script ignore `.curlrc`. Les fichiers sont placés dans un répertoire temporaire privé et supprimés à la sortie. Le checksum est contrôlé avant toute extraction ou exécution ; seuls les trois binaires, l'installateur et l'utilitaire de configuration sont extraits comme fichiers réguliers vers des destinations fixes. Un échec de téléchargement, un checksum incorrect ou une archive incomplète ne remplace pas l'installation existante. Les anciennes releases sans utilitaire restent installables. SHA-256 protège l'intégrité du transfert, sans remplacer la confiance dans le dépôt choisi et dans le script initial.

### Variante pour un fork privé

L'installation du dépôt public AgentRun ne nécessite pas GitHub CLI. Pour un fork privé, remplacer `OWNER/REPOSITORY` par son dépôt et utiliser GitHub CLI installé et authentifié (`gh auth login` si nécessaire) :

```bash
agentrun_repo=OWNER/REPOSITORY
gh release download --repo "$agentrun_repo" --pattern install-agentrun.sh --output install-agentrun.sh &&
sh install-agentrun.sh --repo "$agentrun_repo" --github-cli
```

Le compte connecté doit avoir accès au dépôt. `--github-cli` utilise l'authentification de `gh` sans afficher de token ni le passer dans les arguments ; ce mode convient aussi à un dépôt public. Un dépôt privé renvoie généralement 404 aux téléchargements anonymes. GitHub CLI refuse d'écraser un script local existant sans son option `--clobber` ; un script déjà téléchargé peut aussi être réutilisé directement avec `sh install-agentrun.sh ...`. Le script reste indépendant du propriétaire du dépôt, sélectionné explicitement par `--repo`.

## Choisir le téléchargement

Ouvrir les [releases publiques AgentRun](https://github.com/Lucashw68/AgentRun/releases/latest) et choisir la dernière version. Télécharger l'archive et son fichier `.sha256` :

| Résultat de `uname -m` | Archive |
| --- | --- |
| `x86_64` | `agentrun-0.3.3-x86_64-unknown-linux-musl.tar.gz` |
| `aarch64` ou `arm64` | `agentrun-0.3.3-aarch64-unknown-linux-musl.tar.gz` |

Les fichiers `Source code` générés par GitHub ne contiennent pas les exécutables. Choisir les assets nommés ci-dessus. Les architectures 32 bits ne sont pas distribuées.

## Compatibilité du noyau

**Linux 6.9 minimum**, `/proc` accessible et appels pidfd autorisés sont nécessaires. Vérifier `uname -r`. La liaison statique supprime la dépendance à une version de glibc ; elle n'ajoute pas les fonctionnalités absentes du noyau.

| Distribution | Conditions |
| --- | --- |
| Ubuntu, Linux Mint et dérivées | Noyau actif 6.9+. Ubuntu 24.04 avec un HWE récent convient ; le noyau GA 6.8 ne convient pas. Ubuntu 22.04 avec son HWE 6.8 reste incompatible. |
| Fedora | Noyau actif 6.9+, y compris dans les environnements où seccomp restreint les appels système. |
| Arch Linux, Manjaro et dérivées | Noyau standard ou LTS actif 6.9+ ; vérifier les installations qui conservent un ancien LTS. |
| Debian | Noyau actif 6.9+ ; une installation avec un ancien noyau nécessite une mise à niveau prise en charge par la distribution. |
| Alpine Linux | Architecture 64 bits et noyau actif 6.9+ ; aucune couche de compatibilité glibc nécessaire. |
| openSUSE et autres Linux | Mêmes conditions de noyau et d'architecture ; ces distributions ne font pas toutes partie de la matrice de tests. |

Le [cycle des noyaux Ubuntu](https://ubuntu.com/kernel/docs/reference/hwe-kernels/) distingue GA et HWE. AgentRun ne modifie ni n'installe de noyau. Un refus `UNSUPPORTED_KERNEL` reste un refus sûr ; aucune signalisation moins fiable n'est utilisée comme fallback.

## Installation utilisateur, sans sudo

Depuis le dossier de téléchargement, exemple x86_64 :

```bash
sha256sum -c agentrun-0.3.3-x86_64-unknown-linux-musl.tar.gz.sha256
tar -xzf agentrun-0.3.3-x86_64-unknown-linux-musl.tar.gz
cd agentrun-0.3.3-x86_64-unknown-linux-musl
sh ./install.sh
export PATH="$HOME/.local/bin:$PATH"
agentrun --version
agentrun list --json
```

Pour ARM64, remplacer `x86_64` par `aarch64` dans les noms de fichiers. Le checksum vérifie l'intégrité de l'archive ; il ne remplace pas une signature de provenance.

`install.sh` copie les trois binaires et `agentrun-setup` dans `~/.local/bin`. Il vérifie le noyau et l'exécution du binaire avant de copier, refuse les destinations exécutables qui sont des symlinks, et ne télécharge rien. Il ne modifie ni le PATH permanent, ni les fichiers de shell, ni la politique MCP, ni le registre ou les processus en cours. Ajouter `~/.local/bin` au PATH de son shell si nécessaire. Aucune installation globale par apt, dnf ou pacman n'est nécessaire.

Un autre préfixe absolu est possible :

```bash
sh ./install.sh --prefix "$HOME/Applications/agentrun"
export PATH="$HOME/Applications/agentrun/bin:$PATH"
```

Pour une mise à jour, extraire la nouvelle archive et utiliser le même préfixe. Terminer puis relancer les connexions MCP pour charger la nouvelle version. Les processus persistants et leur registre survivent à la fermeture du client. Les configurations existantes sont conservées ; ajouter les nouveaux profils souhaités manuellement.

Les archives restent utilisables sans installation : `./agentrun list --json`, puis indiquer le chemin absolu de `agentrun-mcp` dans le client. Toujours conserver **`agentrun`, `agentrun-mcp` et `agentrun-log` côte à côte**.

## Configuration et agents

Utiliser `agentrun-setup codex` ou `agentrun-setup config`, puis suivre le [guide des agents](agents.md#configuration-automatique-recommandée) pour les détails et les autres clients. Le [catalogue](profiles.md) décrit les 36 profils disponibles. L'installation des binaires n'autorise pas automatiquement le lancement de projets via MCP.

## Compilation depuis les sources

La compilation Cargo normale reste disponible : `cargo install --path . --locked`. Elle cible la libc de l'environnement de compilation. Pour reproduire les binaires statiques, voir les [commandes de la CI](ci.md#reproduire-localement).

## Portée des tests de distribution

La CI exécute les tests Rust natifs sur x86_64 et ARM64. Elle installe ensuite les archives dans des conteneurs Ubuntu 24.04, Fedora 44, Debian 13 et Alpine 3.23 ; Arch est testé sur x86_64. Les tests exercent le CLI et le MCP sans accès réseau dans les conteneurs et avec un utilisateur non privilégié.

Ces conteneurs partagent le noyau du runner. Ils vérifient l'installation et la compatibilité des environnements logiciels, sans certifier les noyaux par défaut de toutes ces distributions ni toutes leurs politiques SELinux, AppArmor ou seccomp. La disponibilité de `pidfd` reste vérifiée par le Core lors d'un lancement.
