# Processus locaux et gestionnaires externes

## Périmètre actuel

AgentRun gère des processus Linux persistants et leur groupe de processus. Il peut retrouver une commande, ses logs et son état, puis signaler le groupe après vérification de l'identité noyau. Il n'a actuellement aucun backend Docker Compose ou systemd.

Avant de proposer un profil, examiner la commande de démarrage existante du composant demandé. Un `npm run dev` peut démarrer un serveur local, mais aussi déléguer à Docker ; un nom de commande seul ne suffit pas. La présence d'un Dockerfile dans le dépôt ne rend pas tous ses composants incompatibles : un frontend local et une base Docker ont des modes de gestion distincts.

| Composant demandé | Comportement attendu |
| --- | --- |
| Serveur ou worker local qui reste dans son groupe | Utiliser le profil AgentRun autorisé, après vérification de la politique |
| Stack pilotée par Docker Compose | Expliquer l'absence de support natif et proposer le workflow Docker existant avec l'autorisation appropriée |
| Service confié à systemd ou à un autre gestionnaire | Expliquer la limite et proposer le gestionnaire existant avec l'autorisation appropriée |
| Lanceur dont le comportement reste incertain | Examiner les sections utiles de sa recette avant de choisir ; ne pas inventer une garantie de gestion |

Ne pas créer de script intermédiaire ni modifier le Makefile, package.json, les fichiers Compose ou `.env` uniquement pour faire passer un workflow existant par AgentRun. Ajouter une racine ou un profil ne transforme pas un gestionnaire externe en processus directement géré. Un refus de politique pour un processus local ne doit pas être contourné en changeant de méthode de lancement.

La détection d'un cas hors périmètre est un constat préalable de l'agent. Si aucun appel de lancement n'a été envoyé, ne pas affirmer qu'AgentRun a rejeté l'appel. Un échec HTTP indique seulement que l'endpoint n'était pas joignable depuis ce contexte à cet instant ; consulter l'état fourni par le gestionnaire avant de conclure que toute la stack est arrêtée.

Ces règles sont incluses dans les consignes installées à partir de **0.3.5**. Après mise à jour, relancer `agentrun-setup codex`, puis ouvrir une nouvelle session. Elles guident l'agent, sans constituer une interdiction technique générale d'exécuter le client Docker dans un profil ni une modification du système d'approbation du client.

## Pourquoi un client Compose attaché ne suffit pas

Le client Docker transmet ses demandes à un daemon qui gère les conteneurs. Enregistrer le PID du client ne revient donc pas à enregistrer l'identité des conteneurs et leur appartenance à une tâche AgentRun. Voir l'[architecture Docker](https://docs.docker.com/get-started/docker-overview/#docker-architecture).

Compose attaché prévoit un arrêt des conteneurs lorsqu'il reçoit SIGINT ou SIGTERM. Cela peut fonctionner dans le cas normal, mais AgentRun ne vérifie actuellement ni les identités des conteneurs ni leur arrêt effectif via Docker. La fin du client, notamment après un SIGKILL, n'est pas une preuve suffisante du résultat sur toute la stack. Voir [`docker compose up`](https://docs.docker.com/reference/cli/docker/compose/up/).

Un tel lanceur peut aussi déclencher des opérations supplémentaires : construction d'images, recréation de conteneurs ou suppression de conteneurs orphelins. Ces effets doivent être compris et autorisés ; ils ne découlent pas du seul besoin d'enregistrer un processus local.

## Évolution possible : un adaptateur Docker Compose

Cette section décrit une **piste de conception, non implémentée**. L'objectif serait de suivre une stack de développement et d'appeler son gestionnaire existant depuis le Core partagé. Compose garderait la responsabilité de la composition et de l'exécution ; AgentRun fournirait l'inventaire, l'attribution et les opérations explicites du CLI/MCP. Aucun serveur réseau AgentRun ni modification des projets ne serait nécessaire.

Un premier support pourrait se limiter à Docker Compose sur un moteur local, avec les exigences suivantes :

1. **Recette explicitement autorisée.** Résoudre les chemins réels des fichiers Compose et de configuration, fixer le projet, les services et le moteur local ciblé, puis distinguer lancement et construction. Préserver les fichiers du projet. Ne pas accepter de shell libre ni de contexte Docker distant implicite.
2. **Registre adapté aux stacks.** Enregistrer une ressource de type stack avec la recette approuvée, l'owner, l'identité du moteur et les identifiants complets des conteneurs suivis. Vérifier aussi les métadonnées Compose ; le seul nom du projet ne doit pas suffire pour une opération destructive. Une stack existante devrait être adoptée explicitement avant de devenir gérable par AgentRun.
3. **État provenant de Docker.** Rapporter l'état des services, leur santé lorsqu'elle existe, leurs ports et leurs logs, y compris quand un seul service est en panne ou que le moteur est inaccessible. Un moteur inaccessible devrait donner un état inconnu, pas la conclusion que la stack est arrêtée. [`docker compose ps`](https://docs.docker.com/reference/cli/docker/compose/ps/) expose notamment des identifiants, états, ports et informations de santé ; Compose ajoute des [labels de projet et service](https://docs.docker.com/reference/compose-file/services/#labels).
4. **Arrêt limité aux ressources enregistrées.** Revalider les conteneurs ciblés, demander leur arrêt à Docker avec son délai de grâce, puis vérifier le résultat. Ne pas arrêter d'autres stacks, supprimer des volumes, nettoyer globalement Docker ou traiter le PID du client comme l'identité des services.
5. **Validation réelle avant distribution.** Tester les collisions de noms, les stacks préexistantes, les conteneurs recréés extérieurement, les démarrages partiels, le client interrompu et le moteur indisponible. Refuser les opérations ambiguës et conserver les ressources non concernées.

L'accès au moteur Docker et les recettes approuvées auraient leur propre modèle de sécurité ; ce support ne constituerait pas un sandbox pour du code non fiable. Ce travail serait une fonctionnalité séparée, avec un schéma de registre et des garanties documentés avant son activation.
