# Docker Compose et Makefiles

Depuis 0.4.0, le Core possède un adaptateur Docker Compose local et des profils Make. CLI et MCP utilisent ce même adaptateur. AgentRun réutilise les fichiers existants ; il ne fabrique pas de script intermédiaire et ne réécrit ni Makefile, ni fichier Compose, ni `.env`.

## Choisir le mode adapté

| Recette existante | Mode AgentRun |
| --- | --- |
| `make dev` reste au premier plan avec le serveur et ses enfants | Profil `make`, outils de processus |
| Fichiers Compose décrivant une stack | Profil `compose`, outils de stack |
| `make prepare` prépare les fichiers ou images nécessaires | `prepareMake` explicite dans un profil Compose |
| `make up` démarre Docker puis quitte | Examiner sa recette ; déclarer ses fichiers Compose et, si elle existe, sa cible de préparation séparée |
| systemd, Swarm, gestionnaire distant ou recette impossible à séparer | Workflow existant avec l'autorisation appropriée ; pas de fausse supervision par PID |

AgentRun ne peut pas déduire les effets de toutes les recettes Make. Une cible locale doit rester au premier plan. Une cible `prepareMake` doit terminer, **sans démarrer de services persistants**. Elle exécute le code existant du projet avec ses effets habituels : création de configuration, compilation, téléchargements éventuels. Cette restriction est un contrat de configuration, pas une analyse ou une isolation du Makefile. Ne pas lui donner une cible `up` qui démarre une autre stack.

## Configuration

Ajouter uniquement les racines et recettes explicitement approuvées à la politique utilisateur. Exemple :

```json
{
  "allowedRoots": ["~/projects", "~/.codex/worktrees"],
  "profiles": {
    "make-dev": {
      "make": {"file": "Makefile", "target": "dev"}
    },
    "application-stack": {
      "compose": {
        "files": ["compose.yml"],
        "envFiles": [".env", ".runtime/services.env"],
        "socket": "/var/run/docker.sock",
        "prepareMake": {"file": "Makefile", "target": "prepare"},
        "build": false,
        "pull": "never"
      }
    }
  }
}
```

Cet exemple suppose que ces fichiers et cette cible existent dans le projet. Omettre `prepareMake` et `envFiles` s'ils ne sont pas nécessaires. La préparation peut produire les `envFiles` manquants ; leurs ancêtres sont résolus avant exécution et leurs chemins réels complets sont contrôlés ensuite. Les fichiers Compose et Makefile doivent déjà exister. Tous les chemins de recette doivent rester dans le cwd réel autorisé, y compris via des symlinks. Les contenus de ces fichiers peuvent référencer d'autres fichiers et ressources : ils restent du code/configuration de confiance.

Un profil contient exactement un champ parmi `command`, `make`, `compose`. Aucun argument, variable Make, nom de cible ou commande libre ne peut être fourni par un appel MCP. Les cibles sont des identifiants simples ; options, `VAR=value`, chemins et traversées sont refusés. AgentRun neutralise les variables de contrôle Make héritées, notamment `MAKEFLAGS`, `MAKEFILES` et `GNUMAKEFLAGS`.

Les images doivent être présentes par défaut. `build: true` autorise explicitement Compose à construire les images ; `pull: "missing"` autorise leur téléchargement lorsqu'elles manquent. Ces options peuvent nécessiter du réseau pour la stack. AgentRun n'installe ni Docker ni Make, ne demande pas sudo et ne change pas l'accès au socket Docker. Un moteur rootless est possible en configurant son socket Unix absolu. Les variables `DOCKER_*` et `COMPOSE_*` héritées sont retirées des appels du backend ; le moteur est fixé par `--host unix://…`, sans contexte distant implicite.

## CLI

```bash
agentrun start web --profile make-dev --cwd /absolute/project
agentrun logs web
agentrun restart web
agentrun stop web

agentrun stack start application --profile application-stack --cwd /absolute/project
agentrun stack list --json
agentrun stack status application --json
agentrun stack logs application --tail 100
agentrun stack stop application
agentrun stack restart application

agentrun list --json
agentrun stop --all
agentrun clean
```

Les sous-commandes `stack` acceptent toutes `--json`. `list` expose deux tableaux, `processes` et `stacks`. `stop --all` traite les deux types et rapporte les échecs individuellement. `clean` ne supprime une stack du registre que si tous ses conteneurs enregistrés ont disparu et qu'aucun conteneur extérieur ne porte son projet. Une stack arrêtée reste enregistrée pour permettre son redémarrage. Aucun nettoyage Docker n'est exécuté.

## MCP et agents

Les sept outils de processus restent disponibles. Six outils s'ajoutent :

```text
list_stacks()
get_stack(id)
start_stack({id, cwd, profile})
stop_stack(id)
restart_stack(id)
get_stack_logs(id, tail?)
```

`list_processes()` inclut aussi `stacks`, afin qu'un agent voie les ressources déjà lancées. Les outils de stack n'acceptent jamais de PID, d'ID Docker fourni librement ou de commande shell. `start_stack` et `restart_stack` appliquent la politique actuelle ; une révocation est vérifiée avant tout arrêt de redémarrage. `stop_stack` reste disponible après révocation pour arrêter les ressources déjà autorisées.

Après mise à jour des binaires, relancer `agentrun-setup codex`, puis ouvrir une nouvelle session. Les consignes installées demandent de choisir le bon type de profil et de conserver les recettes existantes. Les configurations déjà présentes sont conservées : ajouter les nouveaux profils explicitement, après examen du projet et autorisation de l'utilisateur.

## Identité, persistance et arrêt

Chaque lancement utilise un nouveau projet Compose `agentrun-<UUID>`. AgentRun n'adopte pas une stack déjà démarrée sous un autre projet. Cette séparation évite de recréer ou d'arrêter les conteneurs d'un workflow existant. Elle change aussi les noms Docker générés : une recette dépendant d'un nom de projet fixe doit conserver son workflow existant pour le moment.

Le registre conserve la recette, le cwd, l'owner, l'identité du moteur Docker, le projet Compose et les **identifiants complets des conteneurs** avec leur service. L'intention est écrite atomiquement avant `compose create`. Les conteneurs sont créés à l'arrêt ; leurs identifiants sont persistés avant tout `docker container start`. Un arrêt brutal entre création et enregistrement laisse une entrée `unknown` nécessitant une inspection humaine ; aucune adoption automatique n'est tentée. Un échec après démarrage partiel conserve les identités déjà enregistrées pour permettre l'arrêt.

Avant un arrêt, le moteur, les IDs complets et les labels Compose sont revalidés. AgentRun ne cible ni les noms courts, ni un projet entier via `compose down`. Docker reçoit une demande d'arrêt de chaque conteneur enregistré avec cinq secondes de grâce, puis applique sa procédure de terminaison. L'état est relu pour vérifier le résultat. AgentRun ne supprime aucun conteneur, réseau, image ou volume et ne lance jamais `prune`.

Le redémarrage arrête puis relance **les mêmes conteneurs**. Il ne reconstruit pas l'image et ne recrée pas les conteneurs. Une modification des chemins/options du profil impose une nouvelle stack ; le redémarrage ne rejoue pas `prepareMake`. Les dépendances `service_started`, `service_healthy` et `service_completed_successfully` sont respectées au démarrage, avec une attente de 60 secondes pour les conditions. Une stack avec un job terminé peut apparaître `partial` : inspecter les états et codes de sortie des services.

États : `creating`, `running`, `partial`, `stopped`, `missing`, `unknown`. `running` demande que tous les conteneurs soient en exécution et que les healthchecks présents soient sains ; cela ne remplace pas une vérification HTTP applicative. Un moteur indisponible, une identité différente ou des conteneurs extérieurs donnent `unknown`, jamais une fausse conclusion d'arrêt. Les ports rapportés sont les ports TCP publiés par Docker. Les logs viennent du pilote de logs Docker, avec une lecture bornée ; stdout et stderr peuvent être regroupés sans conserver leur entrelacement exact.

Le registre conserve son enveloppe V1 et ajoute `stacks` seulement lorsqu'il en contient. Les anciennes entrées de processus sont lues sans migration manuelle. **Un ancien binaire refuse un registre contenant ce nouveau champ** : ne pas mélanger les versions après le premier lancement de stack.

## Limites et sécurité

- Docker Compose V2 compatible avec `config --format json` et `create --pull` est requis ; tests locaux avec Compose 5.5.1. Aucun daemon réseau AgentRun, HTTP, WebSocket ou port MCP n'est ajouté. Docker utilise un socket Unix local.
- Pas de noms de conteneurs fixes, profils Compose conditionnels, réplication, watch, hooks de cycle de vie, adoption, reconstruction à chaud ou gestion systemd. Les configurations concernées sont refusées avant création.
- Maximum 16 stacks, 64 conteneurs par stack, huit fichiers Compose et huit fichiers d'environnement. Sorties des clients bornées à 1 Mio par flux ; lecture des logs retournés bornée à 1 Mio. Une sortie excessive produit une erreur explicite.
- Appels Docker et préparation Make limités à 20 secondes ; création/construction limitée à cinq minutes. Le verrou du registre sérialise les mutations ; une commande concurrente peut recevoir `LOCK_TIMEOUT` après 30 secondes pendant un build long.
- Docker conserve ses propres logs et ressources après arrêt. Leur rotation et leur suppression restent à configurer dans le workflow Docker existant. AgentRun ne borne pas les logs sur disque du moteur.
- Les profils autorisent du code, ils ne le rendent pas fiable. Une recette Make, un build ou un conteneur avec des montages/privileges dangereux peut agir hors de la stack. L'accès au socket Docker est lui-même puissant. Ce support ne fournit pas de sandbox pour du code hostile ni de protection contre un acteur du même compte capable de modifier la politique ou le registre.

Références : [architecture Docker](https://docs.docker.com/get-started/docker-overview/#docker-architecture), [création Compose](https://docs.docker.com/reference/cli/docker/compose/create/), [démarrage par ID](https://docs.docker.com/reference/cli/docker/container/start/).
