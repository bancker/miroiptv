# Zenderlijst en defecte zenders — ontwerp

Datum: 2026-10-15. Bouwt voort op de M3U-bronnen (`2026-10-14-m3u-bronnen-design.md`, branch
`feat/m3u-bronnen`). Branch: `feat/zenderlijst`, bovenop die van M3U.

## 1. Doel

Een M3U-bron heeft geen gids en geen overzicht. Je zapt op en neer door soms duizenden zenders,
en een gratis lijst als die van iptv-org (ruim 11.000 zenders) zit vol dode links.

Dit ontwerp bouwt twee dingen:

1. `g` opent bij een M3U-bron een **zenderlijst** met groepen. Bladeren erdoor laat de zender
   achter de lijst al spelen.
2. Een zender die niet start, krijgt een **defect-markering**. Hij komt dan niet meer voorbij bij
   zappen en in de lijst, tot hij weer werkt of 7 dagen voorbij zijn.

Geslaagd is het als:
- je met een paar toetsen bij elke zender bent;
- dode zenders vanzelf uit beeld raken, zonder dat een haperende verbinding goede zenders
  markeert;
- de app niet trager wordt: niet bij het opstarten, niet bij zappen, niet in geheugen.

## 2. Wat de gebruiker koos

- **De indeling:** "Groepen + zenders": links de groepen, rechts de zenders van die groep.
- **De toetsen,** in eigen woorden: "scrollen door de lijst start de zender erachter. enter is
  lijst sluiten en die zender spelen".
- **Het ontwerp van de defecte zenders:** "dead channel design is right".
- **De duur van een markering:** "Bewaard, 7 dagen".

## 3. De zenderlijst

### 3.1 Openen en sluiten

- `g` opent de lijst bij een M3U-bron. De melding "voor deze bron is er nog geen gids" vervalt
  daarmee.
- Bij een Xtream-bron blijft `g` de gids, zoals nu.
- Is de catalogus nog niet geladen, dan volgt de bestaande melding dat hij laadt, en geen lege
  lijst.
- `Esc` of nog eens `g` sluit de lijst.
- Zolang de lijst open is, krijgt zij het toetsenbord. De andere sneltoetsen doen dan niets, zoals
  bij de instellingen.

### 3.2 Indeling

De lijst is een venster over het lopende beeld. Het beeld loopt door, want overlays blokkeren
nooit.

*Aanvulling (2026-10-16, beslissing van de gebruiker):* "bij de m3u gids wil ik dat ik meer van het
beeld zie dat de actieve zender is. toon de zender groepen en zenders aan linkerkant en toon het
beeld rechts in het window." De lijst staat daarom links in het venster (ruim 40 % van de breedte,
hooguit 760 px) en het beeld van de zender die speelt rechts ernaast, heel en op schaal, even hoog
als de lijst. Zolang de lijst open is, tekent mpv het beeld op die maat: minder pixels per frame
dan voor het hele venster. De toetsen onderaan de lijst passen in twee regels.

**Links staan de groepen, in deze volgorde:**
1. **Alle zenders.**
2. **Favorieten.** Alleen als deze bron favorieten heeft die in de catalogus staan.
3. De groepen uit `group-title`, A–Z (hoofdletters tellen niet, getallen op waarde).
4. **Zonder groep.** Alleen als er zenders zonder groep zijn.
5. **Defecte zenders.** Alleen als er gemarkeerde zenders zijn.

Elke groep toont haar aantal zenders. Gemarkeerde zenders tellen alleen mee bij Defecte zenders.
Een zender met meerdere groepen (`A;B`) staat in elk van die groepen.

**Rechts staan de zenders van de gekozen groep:**
- A–Z, met getallen op waarde: "Sport 2" vóór "Sport 10".
- De zender die speelt, heeft een ▶.
- Bij Defecte zenders staat achter elke zender hoe lang geleden hij faalde.
- Lange namen worden afgekapt.
- Onderaan staat één regel met de toetsen.
- Alle teksten zijn er in NL, EN en DE.

### 3.3 Waar de lijst opent

- De lijst opent in de groep die je deze sessie het laatst open had. De eerste keer is dat Alle
  zenders.
- Staat de zender die speelt in die groep, dan staat de keuze op hem. Zo niet, dan staat de keuze
  op de eerste zender van de groep.
- Openen start niets.

### 3.4 Toetsen

- **↑/↓** zet de keuze een regel verder, en die zender start achter de lijst.
  - Het tempo is dat van het zappen (`ZapPacer`): één druk start meteen.
  - Snel achter elkaar drukken of de toets vasthouden verplaatst alleen de keuze. De zender start
    waar je stopt.
- **PgUp/PgDn** springt een pagina (de zichtbare regels), **Home/End** naar de eerste of de
  laatste. Beide in hetzelfde tempo.
- Aan het begin en het eind stopt de keuze. De lijst loopt niet rond.
- **←/→** gaat naar de vorige of volgende groep.
  - De keuze gaat naar de zender die speelt als die in de groep staat, en anders naar de eerste
    zender van de groep.
  - Er start niets.
- **Enter** sluit de lijst en speelt de gekozen zender. Speelt die nog niet, of wacht hij nog op
  het tempo, dan start hij nu.
- **Esc** of **g** sluit de lijst. Wat speelt, blijft spelen. Een start die nog op het tempo
  wachtte, vervalt.
- **De muis:**
  - klik op een groep: die groep;
  - klik op een zender: die zender start, zoals één druk;
  - dubbelklik: de zender start en de lijst sluit;
  - het wieltje scrolt, zonder iets te starten.

### 3.5 Daarna

- Zolang de lijst open is, verschijnt er geen zap-melding met de zendernaam: de ▶ toont al wat
  speelt.
- De foutmelding "streamfout op …" verschijnt wel.
- Na het sluiten zapt ↑/↓ op het hoofdscherm verder vanaf de zender die speelt. Het hoofdscherm
  volgt zoals nu de volgorde van het M3U-bestand, niet de volgorde A–Z van de lijst.

## 4. Defecte zenders (alleen M3U-bronnen)

### 4.1 Wanneer een zender verdacht is

Een zender is verdacht als zijn start eindigt zonder eerste beeld:
- mpv sluit het bestand met een fout;
- de load wordt geweigerd;
- de app geeft de start op (het herlaadbudget of de stall-watchdog).

"Zonder eerste beeld" is dezelfde toestand die de app sinds M3U al bijhoudt
(`stream_starting_since`).

Niet verdacht zijn:
- een load die je zelf vervangt door verder te zappen of te scrollen (mpv: `stop`);
- een zender die beeld gaf en daarna wegvalt (die herlaadt, zoals nu);
- een zender van een Xtream-bron.

### 4.2 Bevestiging

Een verdachte zender is pas bevestigd als binnen 2 minuten na zijn fout een **andere** zender een
eerste beeld geeft. Dan werkte de verbinding weer. Een markering krijgt hij daarna alleen als de
controle hieronder het ook vindt.

- Zonder zo'n bevestiging vervalt de verdenking stil. Valt je wifi weg en falen vijf zenders achter
  elkaar, dan wordt er niets gemarkeerd.
- Geeft de verdachte zender zelf binnen die 2 minuten beeld, dan vervalt de verdenking ook.
- Verdenkingen staan alleen in het geheugen. Sluit je de app, dan zijn ze weg.

**Controle vóór de markering.** Is een verdachte zender bevestigd, dan vraagt de app zijn adres
nog één keer op, op de achtergrond, voordat hij de markering krijgt:
- Eén GET met de eigen headers van de zender (User-Agent en Referer, zoals mpv ze meekrijgt),
  met een korte time-out: ongeveer 5 seconden voor de verbinding en de headers. Alleen de status
  telt; de rest van het antwoord wordt niet gelezen, want een live-stream houdt niet op.
- Een status van 400 of hoger (na doorverwijzingen), geen verbinding of geen antwoord binnen de
  tijd: dan krijgt hij de markering. Een status 2xx of 3xx: de verdenking vervalt, er komt geen
  markering.
  - *Aanvulling (2026-10-16, beslissing van de gebruiker):* 429, 503 en 509 tellen niet mee. Dat
    zijn antwoorden van een drukke server ("probeer later"), geen bewijs van een dode stream. De
    verdenking vervalt, er komt geen markering. Ook een https-server die tijdens de TLS-handshake
    ophangt, markeert niets, net als een http-server die ophangt voor zijn antwoord.
- Hooguit één controle per verdachte en hooguit vier tegelijk. Een zender die al gemarkeerd is en
  opnieuw faalt, krijgt zonder controle een nieuwe termijn. Een wissel van bron laat lopende
  controles vervallen. Alleen bij M3U-bronnen.

Waarom: valt de wifi korter dan 2 minuten weg, dan geeft de eerste zender daarna weer beeld, en
zou elke zender die in dat gat faalde een markering krijgen. De controle vraagt hun adres als de
verbinding er weer is. De gebruiker koos hiervoor: "Ja, met controle".

De grens: een playlist die nog 200 antwoordt terwijl de videosegmenten erachter dood zijn, wordt
niet gemarkeerd. Die fout valt naar de veilige kant: liever een dode zender te veel in de lijst dan
een goede kwijt.

### 4.3 Wat een markering doet

- De zender verdwijnt uit het zappen (↑/↓ op het hoofdscherm) en uit de gewone groepen van de
  lijst. Hij staat alleen in Defecte zenders.
- Zoeken (`f`) en de favorietenlijst (Shift+F) tonen hem nog, met het label **defect**. Geen ✕:
  de favorietenlijst heeft al een x-knop om een favoriet te verwijderen.
- Een sneltoets (0–9) speelt hem gewoon. Het hervatten na een start doet dat ook.
- Speelt een gemarkeerde zender (omdat je hem koos of omdat hij hervat), dan zapt ↑/↓ verder vanaf
  zijn plaats in de volgorde van het bestand, langs de gemarkeerde heen.
- Het filter zit in de catalogus, zoals het filter op volwassen inhoud. Zappen, de lijst en zoeken
  lezen één plek, zodat een gemarkeerde zender nergens per ongeluk tussendoor glipt.

### 4.4 Weer werken en verlopen

- Geeft een gemarkeerde zender een eerste beeld, langs welke weg ook, dan is de markering meteen
  weg.
- Faalt hij opnieuw, dan begint de termijn van 7 dagen opnieuw. Een zender die al gemarkeerd is,
  krijgt daarvoor geen bevestiging meer nodig.
- Een markering ouder dan 7 dagen vervalt bij het laden van de catalogus van die bron: bij de
  start, bij een wissel naar de bron, of bij F5. De zender zapt dan weer mee.

### 4.5 Opslag

- De markeringen staan per bron in de config-map: `bad_streams.json` voor bron 1 en
  `bad_streams-<id>.json` voor de andere, op de manier van `favorites.json` / `favorites-<id>.json`.
- De inhoud is `{"version":1,"marks":{"<stream_id>":<unix-seconden>}}`: alleen ids en tijden. Er
  staan geen namen, URL's of wachtwoorden in. Een M3U-adres kan een wachtwoord bevatten, en de ids
  zijn stabiel (een hash van `tvg-id` of naam).
- Het bestand wordt geschreven bij elke wijziging. Dat is zelden, en het bestand is klein.
- Een bron verwijderen verwijdert ook haar markeringen.
- Een onleesbaar bestand betekent: geen markeringen. De app logt dat één keer en overschrijft het
  bestand bij de volgende wijziging.

### 4.6 Log

Alleen ids en namen, zoals de zap-regel; nooit een URL. De regels:
- `m3u: <naam> (stream_id <id>) failed before its first picture`
- `marked defect`, met wat de controle vond (bijvoorbeeld `http 404`, `connect failed`, `timeout`),
  nooit het adres
- `answers again, not marked`
- `plays again, mark cleared`
- `m3u: <n> defect marks expired`

## 5. Snelheid en geheugen

**De groepen en de sortering:**
- Ze worden één keer berekend per versie van de catalogus en van de markeringen, niet per frame en
  niet per toetsdruk.
- Het budget is 20 ms voor 11.000 zenders in een release-build, en minder dan 1 MB geheugen. De
  groepen bewaren indexen, geen kopieën van zenders.
- Een release-test meet het en print de uitkomst.

**De zaplijst:**
- Ook die wordt één keer per versie van catalogus en markeringen opgebouwd, met de markeringen er
  al uit.
- Een toetsdruk kopieert niets. Dat lost ook de kopie per druk op die bij M3U al gemeten wordt.

**Tekenen:**
- Alleen de zichtbare regels worden getekend.
- Een gesloten lijst kost per frame niets: geen werk, geen geheugen.
- Een open lijst laat mpv het beeld op de maat naast de lijst tekenen (aanvulling 2026-10-16, §3.2).
- Er is geen nieuwe thread, en geen netwerk, behalve één controle per bevestigde verdachte (alleen
  bij M3U-bronnen).

**Opstarten:**
- Het enige nieuwe is het lezen van een klein bestand bij het laden van de catalogus.
- De meting tegen de vorige versie (zoals bij M3U) moet gelijk of beter uitkomen.

## 6. Niet in deze ronde

- **De lijst bij Xtream.** De groepsnamen vragen één extra portal-verzoek
  (`get_live_categories`). Kan later.
- **Logo's in de lijst.**
- **Zoeken in de lijst.** Daar is `f`.
- **Zappen binnen de gekozen groep op het hoofdscherm.** Kan later.
- **Zelf markeren of terugzetten.** Een markering ontstaat en verdwijnt alleen vanzelf.
- **Een XMLTV-gids voor M3U.** Die zit in ronde 2 van M3U.

## 7. Testen

**Losse tests zonder scherm.** Ze gaan over:
- de groepen: meerdere groepen, zonder groep, de aantallen, de volgorde op getal, Favorieten,
  Defecte zenders;
- de stappen in de lijst: ↑/↓, PgUp/PgDn, Home/End en ←/→, de randen, en waar de lijst opent;
- de markeringen:
  - verdacht en bevestigd binnen 2 minuten, of niet;
  - weer beeld wist de markering, opnieuw falen vernieuwt de termijn;
  - het verlopen na 7 dagen, met een nepklok;
  - lezen en schrijven, een kapot bestand, de bestandsnaam per bron;
- de controle vóór de markering: 200 geeft geen markering, 404, een gesloten poort of een
  time-out wel; de eigen User-Agent en Referer gaan mee; een wissel van bron tijdens een controle
  laat geen markering na; de markering komt pas als het antwoord binnen is;
- de zaplijst zonder gemarkeerde zenders.

**App-tests** (de bestaande Harness, headless). Ze laten zien dat:
- `g` bij M3U opent op de zender die speelt, en bij Xtream de gids blijft;
- ↓ de volgende zender start in het tempo, en vastgehouden ↓ alleen start waar je stopt;
- ←/→ niets start;
- Enter sluit en speelt, en Esc houdt wat speelt en laat een wachtende start vallen;
- een dode zender gevolgd door een werkende gemarkeerd wordt en daarna bij zappen wordt
  overgeslagen;
- een dode zender zonder werkende erna niet gemarkeerd wordt;
- zoeken naar een gemarkeerde zender die weer speelt, de markering wist;
- de markeringen een herstart overleven en verlopen;
- er geen zap-melding verschijnt terwijl de lijst open is.

**e2e** (nep-portal, groep M):
- `M-list`: `g`, ↓, Enter, met de verwachte `loadfile`-regels;
- `M-defect`: M3U DOOD, daarna M3U 01. DOOD wordt gemarkeerd, en zappen slaat hem daarna over.

**Snelheid:**
- de release-meting van de groepen;
- de opstartmeting tegen de vorige versie.

## 8. Bestanden (indicatief)

| Bestand | Wat |
|---|---|
| `src/channel_list.rs` (nieuw) | groepen, sortering, stappen; geen egui |
| `src/channel_list_ui.rs` (nieuw) | tekent de lijst en geeft acties terug, zoals `guide_ui.rs` |
| `src/bad_streams.rs` (nieuw) | verdenkingen, bevestiging, verlopen, het bestand |
| `src/catalog.rs` | zaplijst en groepen zonder gemarkeerde zenders |
| `src/app.rs` | `g` bij M3U, de toetsen en het tempo, de haken voor "geen eerste beeld" en "eerste beeld" |
| `src/storage.rs` | het pad per bron; verwijderen met de bron |
| `src/i18n.rs` | de teksten in NL, EN en DE |
| `rust/e2e/*` | de scenario's in groep M |
| docs | `CLAUDE.md`, `rust/README.md`, `docs/VALIDATION.md`, de README van de zip |

## 9. Volgorde

1. Eerst wordt de M3U-branch afgemaakt: de laatste taak, de eindreview en de venstertests.
2. Daarna komt deze branch, bovenop die van M3U, zodat beide in dezelfde release kunnen.
3. Dan de audio-offset.
4. Dan de LGPL-build van libmpv.

De versie gaat hier niet omhoog: dat beslist de gebruiker bij de release.
