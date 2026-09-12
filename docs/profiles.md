# Profils de développement fournis

[examples/config.json](../examples/config.json) fournit **38 profils** prêts à copier dans la configuration utilisateur. La racine initiale reste `~/.codex/worktrees`. Le [guide des agents](agents.md#2-configurer-les-racines-et-profils-agentrun) explique l'installation sans écraser une configuration existante.

Ce catalogue couvre les usages courants ; il ne peut pas deviner les scripts, points d'entrée ou versions de chaque projet. Un profil inconnu reste refusé. Les profils ne sont pas activés automatiquement dans une configuration déjà installée : y reporter les entrées souhaitées manuellement. Aucun paquet ni runtime n'est installé par AgentRun.

## JavaScript et TypeScript

| Profils | Commandes et prérequis |
| --- | --- |
| `npm-dev`, `npm-start`, `npm-preview` | `npm run dev`, `npm run start`, `npm run preview` ; script correspondant dans `package.json`. |
| `pnpm-dev`, `pnpm-start`, `pnpm-preview` | `pnpm dev`, `pnpm run start`, `pnpm run preview` ; scripts du projet. |
| `yarn-dev`, `yarn-start`, `yarn-preview` | `yarn run dev`, `yarn run start`, `yarn run preview` ; scripts du projet. |
| `bun-dev`, `bun-start`, `bun-preview` | `bun run dev`, `bun run start`, `bun run preview` ; scripts du projet. |
| `vite`, `vite-preview` | Vite, dont les projets React, Vue, Svelte et SvelteKit qui l'utilisent ; `vite` / `vite preview`. |
| `next-dev`, `next-start` | Next.js ; `next dev` / `next start`. |
| `nuxt-dev` | Nuxt ; `nuxt dev`. |
| `astro-dev`, `astro-preview` | Astro ; `astro dev` / `astro preview`. |
| `angular-dev` | Angular CLI ; `ng serve`. |
| `nest-dev` | Nest CLI ; `nest start --watch`. L'adresse d'écoute dépend de l'application. |

Les profils directs utilisent `npx --no-install` : l'outil doit déjà être disponible localement ou dans le cache npm ; aucune installation implicite n'est autorisée par cette option. Les serveurs directs Vite, Next, Nuxt, Astro et Angular reçoivent une adresse explicite `127.0.0.1`. Les profils `*-preview` et `next-start` supposent un build préalable adapté au projet. Les profils de scripts restent préférables quand `package.json` contient des options particulières.

Références : [scripts npm](https://docs.npmjs.com/cli/using-npm/scripts/), [npx](https://docs.npmjs.com/cli/v11/commands/npx/), [Vite](https://vite.dev/guide/cli), [Next.js](https://nextjs.org/docs/app/api-reference/cli/next), [Nuxt](https://nuxt.com/docs/4.x/api/commands/dev), [Astro](https://docs.astro.build/en/reference/cli-reference/), [Angular](https://angular.dev/tools/cli/serve).

## Python

| Profil | Point d'entrée et prérequis |
| --- | --- |
| `python-http` | Serveur de fichiers du répertoire courant, `python3 -m http.server`, sur `127.0.0.1:8000`. |
| `django-dev` | `manage.py runserver`, sur `127.0.0.1:8000` ; Django installé. |
| `flask-dev` | Module `app` détectable par Flask ; rechargement activé, débogueur interactif non demandé. |
| `uvicorn-dev` | Objet ASGI `app` dans `main.py` (`main:app`) ; Uvicorn installé. |
| `fastapi-dev` | Application dans `main.py` ; exécutable `fastapi` installé et visible dans le PATH. |
| `streamlit-dev` | `app.py` ; Streamlit installé, mode headless, collecte de statistiques désactivée. |
| `celery-worker` | Application Celery importable sous `app` ; broker configuré et accessible. |

Les serveurs Python de ce catalogue écoutent explicitement sur `127.0.0.1`. Les modules sont chargés par le `python3` du PATH du serveur MCP ; activer l'environnement virtuel avant de démarrer le client ou configurer un chemin absolu vers son interpréteur. AgentRun n'active pas automatiquement `.venv` et ne résout pas les dépendances.

Références : [Django](https://docs.djangoproject.com/en/5.2/ref/django-admin/#runserver), [FastAPI CLI](https://fastapi.tiangolo.com/fastapi-cli/), [Streamlit](https://docs.streamlit.io/develop/api-reference/cli/run).

## Autres environnements

| Profil | Commande et prérequis |
| --- | --- |
| `cargo-run` | `cargo run --locked` ; projet Rust exécutable avec `Cargo.lock` à jour. |
| `go-run` | `go run .` ; package Go `main` dans le répertoire courant. |
| `dotnet-watch` | `dotnet watch --non-interactive run` ; projet .NET détectable dans le répertoire courant. |
| `rails-dev` | `bundle exec rails server --binding 127.0.0.1` ; gems du projet installées. |
| `laravel-dev` | `php artisan serve --host=127.0.0.1` ; dépendances Composer installées. |
| `php-server` | `php -S 127.0.0.1:8000 -t public` ; répertoire `public` existant. |
| `spring-boot` | `mvn spring-boot:run` ; JDK, Maven et plugin Spring Boot configurés dans `pom.xml`. |
| `quarkus-dev` | `mvn quarkus:dev` ; JDK, Maven et projet Quarkus configuré. |

Références : [dotnet watch](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-watch), [Rails](https://guides.rubyonrails.org/command_line.html).

## Choix et limites

Un profil dont le runtime n'est pas installé ne bloque pas les autres profils. Son lancement échoue avec `EXECUTABLE_NOT_FOUND`, ou le programme écrit son erreur dans les logs si c'est un module, fichier ou script qui manque. Un lancement enregistré n'est pas une garantie que l'application soit prête : consulter `get_process` et `get_logs`.

Les commandes exactes sont dans le JSON. Pour une autre entrée (`src.main:app`, un worker particulier, un port distinct), créer ou modifier un profil dans la politique utilisateur. MCP n'accepte aucun argument supplémentaire. Deux worktrees peuvent utiliser le même profil avec des IDs distincts, mais un port fixe peut être occupé : choisir des profils avec des ports distincts si nécessaire.

Ces profils autorisent l'exécution de code dans les racines approuvées ; **ils ne constituent pas une isolation**. Les scripts npm, compilateurs, plugins et applications conservent les droits et l'environnement du client. Cargo, Go, Maven ou d'autres outils peuvent télécharger des dépendances. L'adresse d'écoute des scripts et applications Rust, Go, .NET, Java et Nest dépend de leur propre configuration. Seul le serveur MCP AgentRun lui-même n'ouvre aucun port réseau.

## Make et Docker Compose

`make-dev` sélectionne la cible `dev` du `Makefile` existant. Elle doit rester au premier plan ; utiliser `start_process` ou `agentrun start --profile make-dev`.

`compose-dev` sélectionne `compose.yml`, sans build ni téléchargement d'image implicite. Utiliser `start_stack` ou `agentrun stack start --profile compose-dev`. Adapter explicitement les fichiers et options aux recettes du projet. Docker et Compose sont requis uniquement pour ce profil.

Voir le [guide Compose et Makefiles](service-managers.md), notamment pour les fichiers d'environnement générés par une cible de préparation existante.
