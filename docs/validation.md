# Validation AgentRun 0.3.0

Validation locale effectuée le 12 septembre 2026, sous Linux 6.12, x86_64, avec Rust 1.98.1. Les registres, configurations et processus de test utilisent des répertoires temporaires. Aucun état utilisateur existant n'a été modifié.

## Vérifications fonctionnelles

- **40 tests Rust réussis** : 6 tests unitaires, 7 CLI, 21 Core, 6 MCP. Ce total inclut deux fonctions auxiliaires de tests de crash/verrou.
- `cargo fmt --all -- --check`, Clippy avec `-D warnings`, build release et création du paquet Cargo validés.
- CLI release réellement exercé : `start`, `restart`, `status`, `stop`, `clean`, puis registre vide.
- MCP stdio réellement exercé ; aucune socket détenue par le serveur natif pendant les vérifications.
- Les tests nécessitant un listener TCP local ont été exécutés hors du sandbox, qui interdisait ce listener. Il appartient au worker de test, pas au MCP.

Les nouveaux scénarios couvrent l'arrêt de tout le groupe avant redémarrage, la conservation de la recette et des anciens logs, le refus d'une identité altérée, la persistance d'une entrée morte après échec d'exécution, la revalidation d'un profil révoqué/modifié, le refus de relancer une commande CLI libre via MCP, les limites de registre/configuration/argv, les trames stdio sans délimiteur, les requêtes en vol, la saturation des opérations Core, la rotation des logs après la fin du CLI et la protection des logs encore verrouillés contre leur suppression.

L'échec de création du thread de récupération est injecté uniquement dans un test unitaire ; l'enfant lancé, son arrêt et sa récupération sont réels. Les autres tests de processus utilisent principalement de vrais programmes Linux.

## Test prolongé natif

Commande :

```bash
python3 scripts/resource_soak.py --cycles 1000
```

Chaque cycle exécute via MCP un démarrage, une lecture de logs, un redémarrage, un arrêt puis un nettoyage. Cela représente **2 000 lancements gérés**, plus leurs collecteurs, dans une même connexion MCP.

| Mesure | Départ | Après 1 000 cycles |
| --- | ---: | ---: |
| Mémoire résidente du MCP | 5 712 Kio | 6 736 Kio |
| Descripteurs ouverts | 3 | 3 |
| Threads | 5 | 7 |
| Enfants restant au point de mesure | 0 | 0 |

La mémoire était de 6 732 Kio à 800 et 900 cycles. Après stabilisation finale, aucun enfant restant, registre vide, 20 générations de logs non référencées conservées. Aucun processus géré par ce scénario n'est laissé actif.

Ces valeurs concernent le serveur MCP ; elles n'incluent pas la consommation des programmes lancés. La RSS comprend les réserves de l'allocateur et du runtime ; une faible croissance ne prouve à elle seule ni une fuite ni son absence.

## Instrumentation mémoire

### AddressSanitizer et LeakSanitizer

Les trois exécutables et leurs dépendances Rust ont été recompilés dans un répertoire temporaire avec `-Zsanitizer=address`, `-Cforce-frame-pointers=yes` et une cible Linux explicite. Cette expérience a utilisé `RUSTC_BOOTSTRAP=1` sur la toolchain disponible ; le build distribué reste un build Rust stable ordinaire. La bibliothèque standard précompilée et la libc n'ont pas été reconstruites avec instrumentation.

**100 cycles complets supplémentaires ont réussi**, avec `ASAN_OPTIONS=detect_leaks=1:halt_on_error=1`, fermeture de stdin et sortie normale du MCP. Aucun rapport ASan/LSan n'a été produit. Les trois descripteurs initiaux sont retrouvés, aucun enfant ne reste, le registre est vide et la rétention respecte les 20 générations.

La première tentative dans le sandbox ne pouvait pas effectuer l'inspection des threads nécessaire à LeakSanitizer. Les résultats retenus proviennent de l'exécution hors sandbox, où cette inspection a pu terminer. La consommation mémoire d'un exécutable instrumenté n'est pas comparable à celle du binaire release.

Pour reproduire sur une toolchain nightly compatible, dans un répertoire de build séparé :

```bash
CARGO_TARGET_DIR=target/asan RUSTFLAGS='-Zsanitizer=address -Cforce-frame-pointers=yes' \
  cargo +nightly build --target x86_64-unknown-linux-gnu
ASAN_OPTIONS=detect_leaks=1:halt_on_error=1 \
  python3 scripts/resource_soak.py --bin-dir target/asan/x86_64-unknown-linux-gnu/debug --cycles 100
```

Voir les [instructions Rust sur les sanitizers](https://doc.rust-lang.org/unstable-book/compiler-flags/sanitizer.html) pour leur portée et les options d'instrumentation de la bibliothèque standard.

### Valgrind Memcheck

Valgrind 3.27.1 a été compilé et installé dans un préfixe temporaire, avec les symboles correspondant au chargeur système, sans modification de la libc installée.

Le collecteur `agentrun-log` a reçu **12 Mio de sorties**, effectué plusieurs rotations, conservé deux segments d'au plus 1 Mio, puis terminé normalement sur EOF. Memcheck rapporte :

- 0 erreur ; 0 octet perdu définitivement, indirectement ou potentiellement ;
- 8 736 octets encore accessibles dans 2 allocations à la sortie ;
- uniquement les 3 descripteurs hérités à la sortie.

Le scénario MCP complet sous cette version de Valgrind est **non validé** : elle ne gère pas le syscall Linux 424, `pidfd_send_signal`, et renvoie `ENOSYS`. AgentRun conserve ses contrôles et refuse le lancement. Aucun fallback de signalisation n'a été ajouté pour contourner cette limitation. Le scénario complet instrumenté est couvert ci-dessus avec ASan/LSan.

## Portée

Ces résultats renforcent la confiance dans les chemins testés ; ils ne constituent ni un audit indépendant, ni une preuve d'absence de toute fuite ou faille. Les tests ne remplacent pas un audit des dépendances, du code unsafe et des courses face à un adversaire du même UID.

Les profils restent du code de confiance exécuté avec les droits et l'environnement du client. Les limites AgentRun ne constituent pas une isolation CPU/RAM/disque de ces programmes. Les descendants quittant leur groupe ou conservant le pipe de logs peuvent survivre ; les limites correspondantes sont documentées dans le README. Les anciens processus à logs directs nécessitent un redémarrage pour bénéficier du collecteur borné.
