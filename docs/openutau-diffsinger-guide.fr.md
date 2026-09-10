---
title: "Prononciation et réglages DiffSinger dans OpenUtau"
type: guide
created: 2026-09-09
---

# Prononciation et réglages DiffSinger dans OpenUtau

## Activer les corrections dans Verse avant l'export

Dans l'écran principal, sélectionner **French Millefeuille** dans
**Pronunciation**, puis exporter à nouveau depuis la partition source. Le choix
est conservé au redémarrage de Verse, après acceptation de la nouvelle analyse.
Le profil **Default — no pronunciation fixes** ne génère aucune indication
phonétique française. Choisir ensuite **DIFFS FR MILLE** ou une voix française
dans OpenUtau ne réapplique pas les corrections de Verse à cet ancien fichier.

Un export corrigé contient par exemple `rêves[fr/r fr/ae fr/v]` : le `s` écrit
reste dans le mot, mais aucun phonème `fr/s` n'est ajouté. Un découpage chanté
comme `rê / ê / ves` conserve ses attaques et son « e » final chanté, avec des
indications distinctes. Les consonnes réellement prononcées de `trace`,
`espace` ou `laisse` restent présentes. La partition originale n'est pas modifiée.

Réexporter dans un nouveau fichier pour conserver les retouches déjà faites
dans OpenUtau. L'ancien projet ne se met pas à jour quand Verse est mis à jour.

## Choisir un phonémiseur compatible

Un phonémiseur traduit les paroles vers les symboles attendus par une banque.
La langue française ne suffit pas à établir cette compatibilité.

Le pack DiffSinger UFR Millefeuille utilise **DIFFS FR MILLE** pour le français.
Les phonémiseurs **FR CVVC**, **FR VCCV** et **FR SPHINX** destinés à des banques
classiques ne constituent pas des remplacements pour ce modèle. Une erreur
`Phoneme "hou" isn't supported` ou `Phoneme "- ch" isn't supported` peut ainsi
signaler que le moteur reçoit un mot ou un alias classique au lieu d'un symbole
DiffSinger. Ajouter ce mot au fichier du modèle ne corrige pas son entraînement.

Le phonémiseur français DiffSinger est intégré à OpenUtau. Le dictionnaire
`cmudict-fr.txt` demandé par la documentation FR CVVC concerne ce phonémiseur
classique. Son absence ne démontre pas une installation incomplète de DiffSinger.

Dans Verse, choisir le profil explicite correspondant à la langue et à la
convention de la banque. Le profil français écrit des indications `fr/…` ; le
profil anglais ARPAbet utilise `en/…`. Une banque multilingue doit accepter les
symboles de la langue sélectionnée : DiffSinger ne garantit pas, à lui seul,
une détection automatique de toutes les langues.

Les indications manuelles entre crochets sont conservées. Un mot absent du
lexique ou un découpage ambigu est signalé ; un dictionnaire plus grand ne
résout pas à lui seul les homographes, les liaisons ou les choix de « e » chanté.

Le profil français retire les tirets typographiques de séparation placés aux
extrémités des syllabes dans l'export USTX, même sans lecture connue :
`zyx- / -qwv` devient `zyx / qwv`, avec le diagnostic de prononciation non
résolue. Sans lecture lexicale, la casse, les espaces et la ponctuation restent
intacts. Les tirets internes (`arc-en-ciel`), les marqueurs isolés (`-`, `+`,
`+~`), les alias forcés (`?alias-`) et les indications manuelles entre crochets
restent inchangés.

Les découpages reconnus, comme `chan- / ger`, `pres– / se`, `mê— / me` et
`rê- / ves`, conservent une attaque et une indication vérifiée sur chaque note.
Ce nettoyage d'écriture préserve les paroles sources et leurs métadonnées,
les notes, le tempo, le pitch et le vibrato. Il s'applique aux exports directs
et aux projets des bundles ; les chemins Default et SVP gardent leur
comportement existant. [FR-003](../_bmad-output/implementation-artifacts/spec-fr-003-syllable-hyphen-cleanup.md) ne
prétend pas résoudre les fragments inconnus ni valider leur rendu à l'écoute.

Le contexte écrit permet aussi de corriger les lectures de fragments : `rai`
dans `j'i / rai`, `son` dans `rai / son`, ou `chan` dans `chan / ger` ne sont
pas traités comme des mots indépendants. Les découpages reconnus conservent
les voyelles répétées et les « e » chantés sur leurs notes : `gar / de / rai`,
`rê / ê / ve` ou `pre / sse / e`. Ce dernier découpage reçoit un schwa sur
`sse` puis un schwa répété sur `e` ; cette règle précise ne s'étend pas à tous
les « e » isolés. `Ouh` reçoit la lecture `fr/ou`.

Un `End` prématuré n'est accepté qu'aux positions prévues dans un découpage
vérifié. Une liaison ne traverse jamais un silence ou une ponctuation.
Un mot explicitement lié mais séparé par un silence peut recevoir des
indications indépendantes ; ses durées restent intactes et aucun `+` ne
traverse le silence. Après `laisse, / se`, la consonne du premier mot reste
en place et la seconde note reçoit sa propre lecture `fr/s fr/ee`.

Quand un accord a été réparti entre plusieurs pistes, Verse peut retrouver
le contexte à partir de la partie, de la portée, de la voix écrite, du verset,
de la répétition et de l'identité des paroles. Les copies doivent appartenir
au même accord écrit et leur succession doit être sans ambiguïté. Ce traitement linguistique conserve les notes et les paroles sur leurs pistes.
La réparation de tenues décrite plus bas peut, séparément, replacer une note
dans la piste technique de son propriétaire prouvé. Un numéro devant une parole, comme
`2.Et`, est omis dans l'indication de prononciation uniquement s'il correspond
au verset explicite ; le texte source reste intact.

La vérification initiale [FR-004](../_bmad-output/implementation-artifacts/spec-fr-004-contextual-sung-readings.md)
résout 480 des 482 avertissements de référence et corrige 51 lectures déjà
appliquées mais incorrectes. Les deux `3.Et` associés au verset 1 restent
signalés. Les 3 124 notes gardent leurs données musicales ; l'allocateur
Millefeuille installé conserve les 3 107 attaques attendues sans symbole
rejeté. Pour une parole contenant deux voyelles sur une note, les deux restent
dans le même groupe phonétique et une tenue suivante reste une tenue.
Ces nombres décrivent cet export de référence, avant les réparations de tenues
suivies dans [FID-002](../_bmad-output/implementation-artifacts/spec-fid-002-source-sung-continuity.md).
Cette vérification ne valide ni le minutage interne des phonèmes ni le rendu
à l'écoute.

Sources : [OpenUtau — phonémiseurs](https://github.com/openutau/OpenUtau/wiki/Phonemizers),
[Millefeuille](https://github.com/imsupposedto/Millefeuille-DiffSinger-French),
[pack UFR](https://utaufrance.com/telechargement-du-pack-diffsinger-ufr/).

Pour le son « un », Millefeuille distingue une convention fusionnée `fr/in`
(« vin » et « un ») et le symbole `fr/un` pour /œ̃/ distinct. Le phonémiseur
installé choisit déjà `fr/in` pour le mot `un`, sans intervention de Verse.
Le pack UFR testé accepte aussi l'indication manuelle `un[fr/un]` : ses symboles
et son attaque ont été vérifiés, sans comparaison d'écoute. Cette indication
permet un essai ponctuel ; elle ne justifie pas de modifier automatiquement le
dialecte de tout le projet. Voir la [table phonétique Millefeuille](https://github.com/imsupposedto/Millefeuille-DiffSinger-French/blob/main/Millefeuille_Phonemes.md).

## Tenues et syllabes sur les deux partitions françaises

La comparaison des sources a confirmé trois notes à rétablir : deux dans le
projet PB et une dans le projet chant. Les deux projets retrouvent la tenue
finale de « Même » dans la seconde piste technique d’Alti ; le PB retrouve aussi
la tenue après « murs ». Verse conserve les notes séparées, avec leurs hauteurs,
durées et identités sources. Il choisit leur piste à partir de la liaison écrite
et des paroles, sans ajouter une voix ni combler un silence.

Au passage suivant, une syllabe réellement écrite sur cette note garde son
attaque. Une note liée à la précédente par une liaison de prolongation conserve
le contexte d’intensité de sa note de départ ; elle ne doit pas consommer un
accent destiné à la prochaine attaque. Une prolongation de syllabe sans cette
liaison de notes garde sa propre attaque.

Cinq autres notes sans paroles du PB restent dans la source et les pistes
d’accompagnement. La partition ne fournit pas de lien suffisant pour leur
inventer une parole. Ces corrections de continuité sont suivies dans
[FID-002](../_bmad-output/implementation-artifacts/spec-fid-002-source-sung-continuity.md).

## Hauteur et timbre

La hauteur est portée par les notes du piano roll. B3, E4 ou E5 désignent des
hauteurs musicales, pas un genre de voix. Déplacer toutes les notes d'une octave
change la musique et doit rester une décision d'arrangement.

Sur les deux partitions françaises examinées, la comparaison des hauteurs et
des instants des notes avec la source ne montre pas de décalage automatique
d'octave. Cinq courts rendus de contrôle ont aussi retrouvé les hauteurs
attendues. Cela ne constitue pas une validation sonore de toute la chanson.
Une voix peut sembler plus claire ou plus aiguë par son timbre même lorsque
la note chantée est correcte.

**GEN** est l'expression classique « gender ». Pour une banque DiffSinger qui
prend en charge le décalage de timbre, OpenUtau utilise la courbe **GENC**.
Modifier la valeur par défaut de GEN n'applique donc pas nécessairement l'effet
attendu au rendu DiffSinger. GENC modifie le timbre ; il ne transpose pas les notes.

Pour tester : ouvrir une partie vocale, choisir **GENC / gender (curve)** dans
une ligne d'expressions sous le piano roll, puis dessiner une valeur sur un court
passage. Commencer par une variation modérée et comparer avec zéro. Si GENC n'est
pas proposé, vérifier les expressions du projet et le support de la banque.
Modifier uniquement la définition globale d'une expression ne remplace pas une
courbe déjà dessinée sur une partie.

Source : [OpenUtau — support DiffSinger](https://github.com/openutau/OpenUtau/wiki/DiffSinger-support).

## Pitch, vibrato et fondus

Les points de portamento par défaut d'OpenUtau ne sont pas la preuve qu'une
partition contenait une courbe de pitch. Dans l'export Verse de référence, la transition
standard comporte deux points à −40 et +40 ms et un vibrato désactivé.
Ces valeurs sont un réglage fixe d'export, pas une courbe retrouvée dans la
partition. Le début négatif place une partie de la transition avant la note ;
son dépassement visuel du mot ne prouve donc pas une erreur d'alignement. OpenUtau
peut ajuster le premier point pour rejoindre la note précédente.

Une courbe de hauteur, un vibrato et un fondu de volume sont trois contrôles
différents. Un silence écrit dans la partition doit rester un silence. Verse
ne doit pas inventer un fondu ou un vibrato absent du fichier pour prétendre à
une conversion identique.

Le transfert [EXP-002](../_bmad-output/implementation-artifacts/spec-exp-002-midi-performance-curves.md)
conserve maintenant les variations explicites de pitch bend et les contrôleurs
MIDI/KAR de volume et d'expression dans les courbes **PITD** et **DYN** d'OpenUtau.
Il tient compte du port, du canal et de la sensibilité de pitch bend déclarée.
Les notes gardent leurs hauteurs et durées ; les passages gouvernés par une
courbe de pitch reçoivent une base plate pour ne pas ajouter le portamento
standard. Les autres gardent le réglage d'export décrit plus haut.

La grille et la plage d'OpenUtau imposent des limites : certaines variations
très brèves ou hors plage sont signalées comme non transférées fidèlement.
Les données d'origine restent conservées. La validation couvre le chargement
et l'échantillonnage par le consommateur OpenUtau installé ; elle ne démontre
pas un rendu acoustique identique avec toutes les voix. Le transfert de ces
expressions vers SVP n'est pas encore actif.

Le volume global d'une piste et les nuances à l'intérieur du morceau restent
deux réglages distincts. Monter toute la piste ne recrée pas un crescendo perdu.
L'import des autres expressions demeure suivi dans
[EXP-001](../_bmad-output/implementation-artifacts/spec-exp-001-expression-fidelity.md).

L'inventaire des 22 partitions distinctes du corpus local a retrouvé des
nuances et des vélocités explicites, mais pas de courbes de bend, de vibrato
ni de soufflets dans les corps de partition examinés. Il faut distinguer
ces données écrites des valeurs générales du style ou de l'instrument.
Leur absence dans ce corpus ne signifie pas que les formats ne peuvent pas
les contenir. Verse transfère maintenant les nuances et soufflets pris en charge dans
la courbe DYN d’OpenUtau, selon [EXP-003](../_bmad-output/implementation-artifacts/spec-exp-003-numeric-score-performance.md).
Sa règle d'interprétation est explicite : `mf` sert de référence, `p` vaut
−7,75 dB et `f` +4 dB. Les valeurs numériques écrites dans la source prennent
la priorité prévue sur les symboles ; une vélocité de note et une nuance ne
doivent pas compter deux fois la même intensité.

Un crescendo ou un diminuendo écrit fournit une durée de transition. Une
indication de disparition au silence permet un fondu vers zéro ; un simple
diminuendo ne signifie pas automatiquement « couper le son ». Ces règles
interprètent la partition et ne garantissent pas la même sensation de volume
entre une banque DiffSinger et l'instrument utilisé dans MuseScore. Dans
OpenUtau, la courbe **DYN** permet d'examiner et de retoucher ce résultat ; le
fader de piste reste disponible pour régler l'équilibre avec l'accompagnement.

Conserver un projet retouché dans OpenUtau sous un nom distinct. Une nouvelle
conversion du fichier source ne réimporte pas automatiquement ces retouches.
Le mode **Live Pitch** peut aussi recalculer le pitch DiffSinger pendant l'édition.

## Couleurs de voix

Les couleurs sont des styles entraînés par le créateur de la banque. Leurs noms
ne définissent pas des effets universels. Pour Mimosa : Core est le style de base,
Dark sombre, Soft doux, Solid ferme, Power puissant, Adult plus mature,
Bored blasé et Rap rappé. « Bored » ne signifie pas « Bear ».

Le dialogue **Ré-organisation des couleurs de voix** réaffecte les anciennes
couleurs aux nouvelles pour toutes les parties vocales de la piste. Une
correspondance proposée par position ne garantit pas que les styles se ressemblent.

Pour le rouvrir, faire un clic droit sur l'en-tête de piste puis choisir cette
commande. Pour modifier seulement un passage, utiliser l'expression **CLR**
dans l'éditeur vocal. `(Default)` revient au choix par défaut de la banque.

Sources : [Mimosa UFR](https://utaufrance.com/voicebank/mimosa/),
[Mimosa AI — créateur](https://mim.utaufrance.com/mimosa-ai/).

## Autres solutions

Millefeuille est une convention phonétique et un système de conversion français,
pas l'unique banque DiffSinger. [TIGER DiffSinger v103](https://github.com/spicytigermeat/tiger_diffsinger/releases)
annonce notamment le français. Il faut vérifier sa documentation, sa licence et
son alphabet avant de la considérer comme un remplacement ; elle n'a pas été
installée ni évaluée à l'écoute dans cet audit.

[eSpeak NG](https://github.com/espeak-ng/espeak-ng) et
[Phonemizer](https://github.com/bootphon/phonemizer) offrent d'autres conversions
texte-vers-phonèmes. Ils nécessitent une adaptation vers les symboles du modèle
et une répartition musicale sur les notes. Ce ne sont pas des phonémiseurs UFR
interchangeables à installer directement. Aucun classement de qualité sonore
n'est établi sans essais d'écoute comparables.
