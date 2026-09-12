# AgentRun

> An agent started something on my machine. I want to know what, where, why, and be able to stop it safely.

AgentRun conserve l'état des processus de développement persistants : serveurs, workers, outils de dev. Après la fin d'une session d'agent ou de terminal, un humain ou un autre agent peut retrouver leurs commandes, répertoires, propriétaires, ports et logs, puis les arrêter avec vérification de leur identité Linux.

**Implémentation entièrement Rust. Linux 6.9+ avec `/proc` et les appels `pidfd` autorisés.** Deux interfaces natives : `agentrun` et `agentrun-mcp`, accompagnées du collecteur interne `agentrun-log`. Aucune dépendance Node.js/npm ou utilitaire `flock` à l'exécution. Aucun daemon réseau, HTTP, WebSocket, dashboard, compte ou port réseau AgentRun.

## Installation

Avec une toolchain Rust compatible avec `Cargo.toml` :

```bash
cargo test --locked --all-targets
cargo build --locked --release
cargo install --path . --locked

agentrun --help
agentrun --version
```

`cargo install` place les trois exécutables dans `~/.cargo/bin` par défaut. Aucun sudo. Autre préfixe utilisateur :

```bash
cargo install --path . --locked --root "$HOME/.local"
export PATH="$HOME/.local/bin:$PATH"
```

Conserver les trois binaires dans le même répertoire : le Core résout le collecteur à côté du binaire, jamais dans le PATH du projet.

Sans installation : `./target/release/agentrun list --json` et `./target/release/agentrun-mcp`. Les binaires compilés n'ont pas besoin d'une toolchain Rust à l'exécution. Ils restent liés à la libc de leur cible de compilation ; ce ne sont pas des binaires universels pour toutes les distributions.

Pour installer Rust, consulter les [instructions officielles Rust](https://rust-lang.org/tools/install/). Le projet ne lance aucun installateur et ne modifie pas votre configuration de shell.

## CLI

```bash
agentrun list
agentrun list --json
agentrun status web-app
agentrun status web-app --json

agentrun start web-app -- pnpm dev
agentrun start api-server -- npm run dev
agentrun start worker -- python worker.py
agentrun start preview --cwd /path/to/projects/web-app --client codex -- pnpm dev

agentrun logs web-app
agentrun logs web-app --tail 100
agentrun stop web-app
agentrun restart web-app
agentrun restart web-app --json
agentrun stop --all
agentrun clean
agentrun ports
agentrun ports --json
```

```text
NAME        STATUS   PID    PORTS  OWNER
web-app     running  12345  3000   manual
api-server  running  12346  5173   codex
worker      dead     -      -      codex
```

- Les options AgentRun précèdent `--`. Chaque argument après `--` est transmis littéralement au programme, sans shell implicite, interpolation ou concaténation.
- `--cwd` sélectionne le répertoire ; sinon le répertoire courant est utilisé.
- Owner vaut `{"type":"manual"}` par défaut. `--client codex` implique `--owner agent`. Un owner `agent` sans client est également accepté.
- IDs : `[a-zA-Z0-9][a-zA-Z0-9._-]*`, 1 à 80 caractères ASCII. Les chemins, NUL, `.` et `..` sont refusés.
- Un ID existant ne peut pas être écrasé par `start`. Utiliser `restart` pour le relancer, ou `clean` après arrêt pour le réutiliser avec une autre commande.
- `stop` est idempotent pour une entrée morte. Un ID inconnu est une erreur. `stop --all` continue après une erreur d'arrêt et la rapporte.
- `clean` retire les entrées mortes ou dont l'identité ne correspond plus, sans envoyer de signal. La rétention des anciens logs reste applicable.
- `logs` affiche les 100 dernières lignes par défaut. `--tail` accepte 0 à 10 000 ; la lecture est limitée au dernier Mio. Pas de suivi continu dans le MVP.
- Le rendu humain retire les séquences de contrôle du terminal. Les logs JSON conservent leur contenu original non fiable.

### Contrat JSON V1

Toutes les commandes ci-dessus acceptent `--json`. Réponse réussie : une ligne stdout, avec `version: 1`.

| Commande | Autres champs |
| --- | --- |
| `list` | `processes: ManagedProcess[]`, triés par ID ASCII |
| `status`, `start`, `restart`, `stop <id>` | `process: ManagedProcess` |
| `ports` | `processes: { id, status, ports }[]` |
| `stop --all` | `stopped: string[]`, `errors: { id, code, message }[]` |
| `clean` | `removed: string[]` |
| `logs` | `id`, `text`, `truncated` |

```json
{"version":1,"processes":[]}
```

Erreur : code de sortie 1, stdout vide, une ligne JSON sur stderr :

```json
{"version":1,"error":{"code":"NOT_FOUND","message":"No registered process: absent"}}
```

Codes principaux : `USAGE`, `VALIDATION_ERROR`, `ID_EXISTS`, `NOT_FOUND`, `IDENTITY_MISMATCH`, `REGISTRY_INVALID`, `UNSAFE_STATE`, `UNSAFE_FILE`, `LOCK_TIMEOUT`, `EXECUTABLE_NOT_FOUND`, `EXEC_FAILED`, `UNSUPPORTED_KERNEL`, `CWD_NOT_ALLOWED`, `UNKNOWN_PROFILE`, `FORBIDDEN_PROFILE`, `CONFIG_ERROR`, `IO_ERROR`, `PROCESS_LIMIT`, `REGISTRY_TOO_LARGE`, `CONFIG_TOO_LARGE`, `RESOURCE_LIMIT`, `LAUNCH_TIMEOUT`, `PROFILE_REQUIRED`, `SERVER_BUSY`, `LOG_HELPER_MISSING`.

Un échec partiel de `stop --all` utilise sa réponse normale et un code de sortie 1. Les clients doivent tolérer de nouveaux champs dans les réponses. L'ordre des clés JSON n'est pas contractuel.

## Architecture

```text
Cargo.toml / Cargo.lock
src/
├── lib.rs                    bibliothèque réutilisable
├── bin/agentrun.rs           CLI clap, rendu humain/JSON
├── bin/agentrun-mcp.rs       transport stdio uniquement
├── bin/agentrun-log.rs       collecteur interne de logs, sans réseau
├── mcp.rs                    sept outils du SDK MCP Rust officiel
├── mcp/transport.rs          budgets stdio avant buffering/dispatch
└── core/
    ├── mod.rs                façade AgentRun et transactions
    ├── types.rs / error.rs   modèle partagé, erreurs structurées
    ├── registry.rs           verrou noyau et écriture atomique
    ├── launch.rs             fork/execve et validation avant exécution
    ├── process_manager.rs    SIGTERM, grâce, SIGKILL via pidfd
    ├── proc.rs               identité noyau, références pidfd, parcours /proc
    ├── port_detection.rs     ports TCP du groupe et des descendants
    ├── limits.rs             budgets de ressources communs
    ├── logs.rs               capture, rotation et rétention bornées
    ├── paths.rs              XDG et fichiers privés
    ├── validation.rs         IDs, commandes et registre
    └── security_policy.rs    profils MCP et allowedRoots
tests/                        tests Rust avec de vrais processus Linux
```

Le CLI et le MCP appellent la même façade `core::AgentRun`. Seul le Core lance, inspecte et signale les processus gérés. Il est synchrone et indépendant de clap, Tokio et MCP. Le serveur exécute ses appels Core dans des tâches bloquantes séparées pour ne pas bloquer le protocole.

Le SDK `rmcp` est compilé avec `default-features = false`, `server` et `transport-io`. Aucun transport HTTP n'est activé. Les blocs `unsafe` sont confinés aux appels Linux nécessaires, documentés et contrôlés avec Clippy ; Rust ne dispense pas de vérifier leur correction.

## Registre, XDG et migration depuis Node

```text
$XDG_STATE_HOME/agentrun/              défaut : ~/.local/state/agentrun/
├── registry.json
├── logs/<id>-<uuid>.log
└── locks/registry.lock

$XDG_CONFIG_HOME/agentrun/config.json  défaut : ~/.config/agentrun/config.json
```

Les variables XDG absentes, vides ou relatives utilisent le fallback dans le répertoire personnel. Les chemins doivent être valides en UTF-8. Le registre et le contrat JSON gardent **la version 1**, compatible avec les entrées du MVP TypeScript. La configuration et les noms de commandes sont conservés. La version courante est `0.3.0`.

La migration du dépôt retire les sources TypeScript, `package.json` et les dépendances npm. Si vous avez installé le précédent package globalement, vous pouvez le désinstaller avec `npm uninstall --global agentrun`, puis installer les binaires Rust. Vérifier `command -v agentrun` pour éviter qu'un ancien exécutable masque le nouveau. Ne jamais supprimer le registre pour changer de langage.

Chaque entrée conserve :

```text
id, pid, pgid, cwd, command[], startedAt, status, ports[],
owner { type: "manual" | "agent", client? },
processStartTime, bootId, uid, logPath, deadReason?, profile?
```

`processStartTime` est une chaîne décimale contenant les ticks du champ 22 de `/proc/<pid>/stat`. Le registre ne contient pas de pidfd : un descripteur est local à un processus, il est rouvert et son identité vérifiée à chaque arrêt. `bootId` empêche une correspondance après reboot.

Chaque opération de consultation ou mutation du registre prend un verrou exclusif natif, revalide les entrées contre Linux et persiste leur état. Les zombies sont morts. Une identité différente donne `dead`, même si le PID existe. Une structure JSON corrompue ou inconnue provoque un refus explicite ; elle n'est jamais remplacée silencieusement.

L'écriture utilise un fichier temporaire unique, `fsync`, `rename` atomique puis `fsync` du répertoire. Le verrou natif `flock(2)` est détenu par un fichier Rust et libéré automatiquement à la sortie ou au crash. Aucun processus auxiliaire de verrou. L'attente est bornée à 30 secondes. Ne pas supprimer le fichier de verrou pendant une utilisation ; le stockage doit être local, pas NFS.

Répertoires créés en `0700`, fichiers en `0600`. Les fichiers doivent être réguliers, appartenir à l'utilisateur, sans symlink ni hardlink supplémentaire. Des permissions existantes trop larges entraînent un refus sans correction automatique.

## Lancement et arrêt sûrs

Le Core prépare les chemins, l'environnement, les logs et des pipes avant `fork`. L'enfant crée une session et un groupe dédiés (`PID = PGID = SID`), adopte le cwd via un descripteur de répertoire, redirige stdin vers `/dev/null` et stdout/stderr vers le pipe du collecteur. Puis il **attend**.

Le parent capture son identité Linux, vérifie la disponibilité des signaux de groupe par pidfd, inscrit l'entrée et synchronise le registre sur disque. Seulement ensuite, un octet de validation autorise `execve`. Le PID, son démarrage, le groupe et la session restent identiques pendant `execve`.

- Parent tué avant validation : fermeture du pipe, l'enfant termine sans exécuter la commande.
- Erreur de persistance : même abandon sans exécution.
- Parent tué après validation : le processus lancé possède déjà une entrée durable.
- Échec d'`execve` : erreur remontée et entrée retirée. Aucun fallback shell.

Les opérations effectuées dans l'enfant entre `fork` et `execve` n'allouent pas de mémoire et utilisent seulement des appels libc sûrs après fork. Le processus lancé survit au CLI ou au MCP. Le MCP récolte ses enfants terminés avec des threads d'attente locaux ; il ne supervise pas leur exécution et n'est pas nécessaire à leur survie.

Avant un arrêt, AgentRun ouvre un pidfd et vérifie PID, PGID, SID, UID, ticks de démarrage et boot ID. Commande et cwd sont conservés pour inspection, mais ne servent pas d'identité immuable : un outil peut modifier son titre ou son répertoire pendant son exécution.

SIGTERM est envoyé au **groupe via `pidfd_send_signal(..., PIDFD_SIGNAL_PROCESS_GROUP)`**. Après 1,5 seconde de grâce, SIGKILL est envoyé par la même référence noyau si le groupe existe encore. Le leader peut disparaître pendant cette période sans perdre la référence au groupe original. Il n'existe aucun fallback `kill(-pgid)` ni aucune interface acceptant un PID externe.

Ce signal de groupe par pidfd est disponible depuis Linux 6.9. Un noyau ou sandbox qui ne l'autorise pas entraîne un refus sûr, avant exécution de la commande lors d'un démarrage. Voir la [documentation Linux de pidfd_send_signal](https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html).

## Redémarrage

`agentrun restart <id> [--json]` réutilise commande, cwd et owner enregistrés. L'arrêt vérifié et le lancement s'effectuent sous le même verrou du registre. Le PID, l'heure de démarrage et le fichier de log changent. Une entrée morte peut être relancée ; si son ancien PID ou groupe existe avec une identité invérifiable, le redémarrage est refusé sans signal.

Le CLI vérifie le répertoire et l'exécutable avant d'arrêter. Un échec du lancement conserve une entrée morte avec sa recette et ses anciens logs pour permettre le diagnostic et une nouvelle tentative. Il n'y a ni retour automatique à l'ancien programme, ni garantie de disponibilité continue. Le nouvel environnement est celui du client actuel ; AgentRun ne persiste pas les variables d'environnement ou les secrets.

`restart_process({id})` exige un champ `profile` enregistré par un lancement MCP. Les anciennes entrées et les commandes CLI libres ne peuvent pas être relancées par cet outil. La configuration actuelle, les racines et le profil sont revalidés **avant l'arrêt**. Un profil révoqué laisse donc l'ancien processus intact ; un profil modifié fournit la nouvelle commande autorisée. L'owner est conservé. Le registre reste V1, avec un champ `profile` optionnel ; les anciennes entrées sont lisibles par cette version, mais un ancien binaire 0.2 peut refuser les nouvelles entrées profilées.

## Détection des ports et logs

Les sockets de `/proc/<pid>/fd` sont jointes par inode aux entrées TCP LISTEN de `/proc/<pid>/net/tcp` et `tcp6`. Le Core parcourt le groupe et ses descendants visibles. Aucun scan réseau, HTTP ou appel à `ss`. Des restrictions `/proc` ou sorties concurrentes peuvent donner une liste incomplète, sans bloquer la commande.

Chaque lancement possède un log UUID distinct, y compris après `restart`. stdout et stderr sont redirigés vers un pipe commun lu par `agentrun-log`. Ce collecteur détaché, sans réseau, continue après la fin du CLI/MCP puis termine à la fermeture de toutes les sorties. Son code appartient au Core ; son exécutable n'accepte aucune commande à lancer.

Chaque génération conserve **deux segments d'au plus 1 Mio** : `id-UUID.log` et `id-UUID.log.1`. Le collecteur copie le segment précédent puis tronque le fichier courant en conservant son inode et son verrou. La capture utilise un tampon de 16 Kio. La lecture combine les segments, reste limitée au dernier Mio et indique `truncated` après rotation. Une lecture pendant rotation reste une observation, pas un instantané transactionnel.

Les opérations sur le registre conservent les logs référencés ainsi que les **20 dernières générations non référencées**. Les plus anciennes sont supprimées automatiquement ; un verrou interdit de supprimer une génération encore utilisée par un collecteur, même après `clean`. Archiver les logs nécessaires avant leur sortie de cette rétention. Les anciens processus lancés avec la version 0.2 continuent d'écrire directement : les redémarrer pour activer la capture bornée.

Une erreur d'écriture fait terminer le collecteur ; le programme peut alors recevoir une erreur de pipe. La rotation ne limite ni le débit produit, ni les autres fichiers que le programme écrit avec ses droits utilisateur.

## Profils et sécurité MCP

Créer le fichier de configuration utilisateur ; aucun profil n'est autorisé par défaut. Sans configuration, consultation et arrêt fonctionnent, mais les démarrages MCP sont refusés.

Le [fichier initial](examples/config.json) fournit **36 profils courants** : npm, pnpm, Yarn, Bun, Vite, Next, Nuxt, Astro, Angular, Nest, Python, Rust, Go, .NET, Rails, PHP et Java. Voir le [catalogue et ses prérequis](docs/profiles.md). Extrait minimal :

```json
{
  "allowedRoots": ["~/.codex/worktrees"],
  "profiles": {
    "npm-dev": { "command": ["npm", "run", "dev"] },
    "pnpm-dev": { "command": ["pnpm", "dev"] },
    "vite": { "command": ["npx", "--no-install", "vite", "--host", "127.0.0.1"] }
  }
}
```

Voir [examples/config.json](examples/config.json). La politique est relue à chaque lancement ou redémarrage MCP, dans le Core. Elle ne restreint pas les commandes explicites du CLI humain.

Cet exemple autorise les répertoires existants sous `~/.codex/worktrees`, y compris les sous-répertoires des projets. AgentRun vérifie les chemins réels, sans vérifier l'appartenance Git ni l'origine Codex du dossier. Si les worktrees sont stockés ailleurs, remplacer cette racine par leur emplacement réel. Pour autoriser aussi un checkout principal situé ailleurs, ajouter explicitement son chemin à `allowedRoots`. Les mêmes profils sont réutilisables dans tous les répertoires autorisés ; un profil inconnu reste refusé avec `UNKNOWN_PROFILE`.

Le MCP exige un cwd absolu. Cwd et racines sont canonicalisés avec résolution des symlinks ; la comparaison respecte les composants de chemin. `/projects-evil` n'appartient pas à `/projects` et un symlink sortant est refusé. Les racines inexistantes n'accordent aucun accès.

Les exécutables des profils sont résolus via chemin absolu ou entrées absolues du PATH du serveur, puis canonicalisés. Shells (`bash`, `sh`, etc.), élévation (`sudo`, `su`, `doas`, `pkexec`) et wrappers `env`/`busybox` sont refusés, y compris les alias symlink pointant vers eux. Les appels MCP ne peuvent fournir ni commande, ni argv, ni environnement, ni nouvelle politique.

**Profils et projets autorisés restent du code de confiance.** Un script npm peut lui-même utiliser un shell, accéder aux fichiers et au réseau, ou lancer d'autres programmes avec les droits de l'utilisateur. Une politique de profils n'est pas un sandbox. Le serveur MCP n'a besoin d'aucun réseau ; les programmes approuvés ont leur propre comportement. `npx --no-install` évite l'installation implicite dans l'exemple Vite.

Le modèle de menace exclut un adversaire contrôlant les fichiers, les exécutables, l'environnement ou les processus du même UID, ainsi que leurs modifications concurrentes hostiles. Le registre privé protège contre les erreurs et les confusions d'identité, pas contre un compte local compromis. Owner est une attribution informative, pas une authentification ; les clients du même compte partagent le registre. Commandes et logs peuvent contenir des secrets.

## MCP stdio

```bash
agentrun-mcp
```

Messages MCP sur stdin, réponses JSON sur stdout, diagnostics sur stderr. Aucun argument de port ou de transport. Les processus gérés peuvent naturellement ouvrir leurs propres ports ; le MCP n'en ouvre aucun.

| Outil | Entrée |
| --- | --- |
| `list_processes` | `{}` |
| `get_process` | `{ "id": "web-app" }` |
| `start_process` | `{ "id": "web-app", "cwd": "/path/to/projects/web-app", "profile": "pnpm-dev" }` |
| `stop_process` | `{ "id": "web-app" }` |
| `restart_process` | `{ "id": "web-app" }` |
| `get_logs` | `{ "id": "web-app", "tail": 100 }` |
| `clean_registry` | `{}` |

Schémas stricts, aucun outil `run_command`. Les résultats reprennent le contrat V1 dans `structuredContent` et dans le contenu textuel JSON. Les erreurs d'outil portent `isError: true`. L'owner vaut `agent`, avec le nom client annoncé au protocole s'il est valide, sinon `mcp`.

Le protocole, son initialisation et sa négociation sont gérés par le [SDK Rust MCP officiel](https://github.com/modelcontextprotocol/rust-sdk).

## Budgets de ressources

Budgets fixes du MVP, partagés par le Core :

| Ressource | Limite |
| --- | --- |
| Entrées du registre, mortes comprises | 64 ; `clean` libère de la place |
| Registre JSON | 8 Mio en lecture et en écriture |
| Configuration MCP | 256 Kio, fichier régulier |
| Commande et arguments cumulés | 64 Kio, au plus 256 arguments |
| Logs d'une génération | 2 × 1 Mio |
| Générations de logs non référencées conservées | 20, hors collecteurs encore actifs |
| Ligne MCP entrante, avant désérialisation | 64 Kio |
| Requêtes MCP en attente de réponse écrite | 16 par connexion |
| Opérations Core MCP simultanées | 8 ; excès refusé avec `SERVER_BUSY` |

Un dépassement du budget de trame ou de requêtes en vol ferme la connexion stdio ; le client doit se reconnecter. Les identifiants de requêtes dupliqués en vol sont également refusés. Le permis d'une opération annulée reste détenu jusqu'à la fin effective du travail bloquant. Aucun de ces budgets ne constitue un quota CPU/RAM des programmes lancés.

La création des threads de récupération des enfants est vérifiée. Si elle échoue, le lancement est abandonné et l'enfant est arrêté/récupéré. Les échanges d'initialisation attendent au plus cinq secondes avant de déclencher le nettoyage ; une tâche bloquée dans le noyau peut néanmoins retarder sa terminaison.

## Intégration Codex et autres agents

Voir le **[guide de connexion des agents](docs/agents.md)** pour l'installation, la politique AgentRun, les instructions permanentes, le test de connexion et le dépannage :

- [Codex : commande d'enregistrement et config.toml](docs/agents.md#codex)
- [Claude Code](docs/agents.md#claude-code)
- [Cursor](docs/agents.md#cursor)
- [VS Code / GitHub Copilot](docs/agents.md#vs-code--github-copilot)
- [Autres clients MCP locaux](docs/agents.md#autre-client-mcp-local)

Après installation dans `~/.local/bin` et configuration de la politique AgentRun :

```bash
codex mcp add agentrun -- "$HOME/.local/bin/agentrun-mcp"
codex mcp list
```

Adapter le chemin si les binaires sont dans `~/.cargo/bin` ou ailleurs. Codex démarre le serveur en stdio. Ouvrir une nouvelle session, puis demander un appel MCP `list_processes` sans démarrer de processus. Voir la [documentation officielle Codex](https://developers.openai.com/codex/mcp/).

**Quand les outils MCP AgentRun sont disponibles, l'agent doit les préférer au CLI.** Ajouter les [consignes MCP du guide](docs/agents.md#4-donner-une-consigne-permanente-à-lagent) aux instructions existantes du client. Les refus de politique ne doivent pas être contournés via le CLI ou une modification silencieuse de la configuration.

Pour un agent configuré explicitement pour utiliser le CLI, le bloc suivant peut être ajouté à ses instructions globales. Il ne remplace pas les consignes MCP lorsqu'une intégration MCP est utilisée :

```md
## Persistent development processes

This machine uses AgentRun to manage persistent development processes.

Before starting a development server, run:

agentrun list --json

Do not directly start persistent processes with:

npm run dev
pnpm dev
vite
nuxt dev

Use:

agentrun start <name> -- <command>

When a server is no longer needed:

agentrun stop <name>

Before completing a task, inspect:

agentrun list --json
```

## Développement et validation

La [CI GitHub](docs/ci.md) vérifie le formatage, Clippy et les tests sur Linux x86_64 et ARM64, puis construit les trois binaires release et fournit des archives avec sommes SHA-256 dans les artifacts de chaque exécution réussie.

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
cargo package --locked --allow-dirty --offline
```

Les tests n'ont besoin ni de Node.js, ni de Python. Ils compilent un petit worker Rust indépendant et utilisent de vrais processus, signaux et connexions stdio. Ils couvrent notamment la persistance avant `execve`, un parent tué par SIGKILL avant validation, l'identité altérée, les enfants résistants à SIGTERM, le leader qui disparaît, la concurrence interprocessus, les écritures atomiques, le verrou après crash, le JSON, les logs, les profils MCP et l'absence de sockets dans le serveur MCP. Ils couvrent aussi les redémarrages, la révocation des profils, les budgets de ressources, la rotation/rétention et l’échec du reaper.

Les tests nécessitent Linux 6.9+, un `/proc` accessible, `rustc`, la création de processus et un listener TCP de test sur `127.0.0.1`. Un sandbox peut interdire ces appels même sur un noyau compatible. Les tests utilisent un état temporaire séparé de votre registre.

## Limites et suite

- Les commandes doivent rester au premier plan dans leur groupe. Un programme qui se daemonise ou quitte son groupe/session échappe à l'arrêt du groupe. Si le leader est déjà mort **avant** une nouvelle commande `stop`, AgentRun refuse d'adopter ses descendants sans identité suffisante ; il marque l'entrée morte. La disparition du leader **pendant** un arrêt déjà engagé est gérée.
- Les pidfd sont rouverts entre sessions et associés à l'identité persistée. Ils ne remplacent pas le registre ni son contrôle d'intégrité, et ne constituent pas une isolation de sécurité du code exécuté.
- Budgets fixes pour ce MVP ; prochaine amélioration possible : des plafonds configurables dans des bornes sûres. Les collecteurs dont un descendant échappé garde le pipe ouvert restent actifs jusqu’à sa fermeture.
- Les ports sont des observations, pas une garantie de readiness. `start` confirme le lancement enregistré ; consulter ensuite `status`, `ports` et `logs`.
- Les arrêts conservent le verrou. Un long `stop --all` peut faire expirer l'attente de 30 secondes d'un autre client, qui peut réessayer.
- Pas de redémarrage automatique, autostart, supervision permanente, GUI, Docker, services système ou gestion multi-utilisateur. AgentRun reste un inventaire local de processus de développement, pas un remplaçant de systemd ou PM2.

### Mesures mémoire reproductibles

Vérification facultative avec Python 3, après build des trois binaires :

```bash
python3 scripts/resource_soak.py --cycles 1000
# Seulement avec un Valgrind qui prend en charge pidfd_send_signal :
python3 scripts/resource_soak.py --cycles 25 --valgrind /path/to/valgrind
```

Chaque cycle lance, consulte, redémarre, arrête et nettoie un processus via MCP. Le script mesure RSS, threads, descripteurs et enfants, vérifie la rétention, puis ferme stdin proprement. Le mode Valgrind vérifie le serveur MCP avec Memcheck ; les programmes exécutés après exec ne sont pas instrumentés par ce mode. Voir [le compte rendu de validation](docs/validation.md) pour les résultats et leur portée.
