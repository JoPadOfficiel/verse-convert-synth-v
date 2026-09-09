---
title: "Prononciation et réglages DiffSinger dans OpenUtau"
type: guide
created: 2026-09-09
---

# Prononciation et réglages DiffSinger dans OpenUtau

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

Sources : [OpenUtau — phonémiseurs](https://github.com/openutau/OpenUtau/wiki/Phonemizers),
[Millefeuille](https://github.com/imsupposedto/Millefeuille-DiffSinger-French),
[pack UFR](https://utaufrance.com/telechargement-du-pack-diffsinger-ufr/).

## Hauteur et timbre

La hauteur est portée par les notes du piano roll. B3, E4 ou E5 désignent des
hauteurs musicales, pas un genre de voix. Déplacer toutes les notes d'une octave
change la musique et doit rester une décision d'arrangement.

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
partition contenait une courbe de pitch. Dans l'export Verse audité, la transition
standard comporte deux points à −40 et +40 ms et un vibrato désactivé. OpenUtau
peut ajuster le premier point pour rejoindre la note précédente.

Une courbe de hauteur, un vibrato et un fondu de volume sont trois contrôles
différents. Un silence écrit dans la partition doit rester un silence. Verse
ne doit pas inventer un fondu ou un vibrato absent du fichier pour prétendre à
une conversion identique. L'import complet des expressions de chaque format
reste suivi dans [EXP-001](../_bmad-output/implementation-artifacts/spec-exp-001-expression-fidelity.md) ; le correctif
de prononciation ne l'implémente pas.

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
