# Bongo Player 0.6.0, deel 1: fundament — ontwerp

**Datum:** 2026-10-11
**Status:** ontwerp, per onderdeel goedgekeurd in de brainstorm van 2026-10-11
**Bron:** `docs/Major update 0.6.0.docx`
**Uitgangspunt:** tag `stable` = `BongoPlayer-v0.5.1` (3a2c10f)

0.6.0 bestaat uit twee deelprojecten, elk met een eigen spec, plan en bouw:

1. **Fundament** (dit document): instellingenscherm, taal NL/EN/DE, adult-inhoud
   achter een pincode, instelbare nieuwsregels.
2. **Nieuwe gids** (later, eigen spec): NLZIET-stijl, live en terugkijken in één
   gids. Gebruikt de vertalingen, het adult-filter en de instellingen van deel 1.

Het versienummer gaat naar 0.6.0 als beide delen klaar zijn.

## Doel

- Alles wat de gebruiker leest, staat in de gekozen taal: Nederlands, English of
  Deutsch. De taal kies je bij de eerste start en wijzig je in de instellingen.
- Eén instellingenscherm voor portal-gegevens, taal, zaprichting, adult-inhoud en
  de nieuwskiezer.
- Adult-zenders en -films komen nooit per ongeluk in beeld (een gezins-pc).
- De nieuwskiezer (toets N) is instelbaar in plaats van ingebakken.

## Vastgestelde feiten (code en portal, 2026-10-11)

- Er is geen vertaalsysteem: ~170 zichtbare teksten in `app.rs`, door elkaar
  Nederlands en Engels, plus foutteksten in `catalog.rs`, `args.rs` en `portal/`.
- Er is geen instellingenmechanisme. Inloggegevens staan in `BongoPlayer.ini`
  naast de exe: de eerste niet-commentaarregel met een `@`, kaal of als
  `sleutel=waarde`. Het portalscherm verschijnt alleen bij ontbrekende of
  placeholder-gegevens en schrijft de ini zonder verbindingstest.
  `user:pass@host` wordt gesplitst op de *eerste* `@`, dus een `@` in het
  wachtwoord breekt het.
- De portal levert per zender en film een betrouwbare `is_adult`-vlag (live: 198
  zenders, waarvan 197 in de categorie "FOR ADULTS"; VOD: 3.937 films, precies
  de categorie "XXX | FOR ADULTS"). Slechts 3 adult-zenders hebben "XXX" in de
  naam, dus herkenning op naam werkt niet. De catalogus bewaart `is_adult` nu
  niet.
- De nieuwskiezer zoekt in 7 vaste bronnen (zender, titeltrefwoord, badge):
  npo1/npo2 "journaal" (NOS, met uitsluitingen sport/jeugd/regio/makkelijke
  taal/gebaren), rtl4 "rtl nieuws" (RTL, start +30 s), cnn en bbcnews (elk
  programma), vrt1 "journaal" (VRT), omroepzeeland "nieuws" (ZLD).
- Zappen: pijl omlaag en wiel omlaag = volgende zender (+1); pijl omhoog en wiel
  omhoog = vorige.
- Elke start logt `resuming last watched (<url>)`, en die URL bevat gebruikersnaam
  en wachtwoord. Direct afgespeelde films loggen hun URL (met inloggegevens) via
  `loadfile` en via mpv's eigen regels.

## Beslissingen (brainstorm 2026-10-11)

- **Bron:** alleen een Xtream-server (server, gebruiker, wachtwoord), aanpasbaar
  in de instellingen, met een verbindingstest. Geen M3U.
- **Adult zonder pincode:** overal verborgen: zappen, zoeken, favorieten,
  sneltoetsen, hervatten en straks de gids.
- **Ontgrendelen:** met de pincode, in de instellingen; geldt tot de app sluit.
  Bij elke start is alles weer op slot.
- **Pincode instellen:** pas bij de eerste keer ontgrendelen, niet bij de eerste
  start.
- **Nieuwsregels:** per regel een zender met titelpatroon(en). De huidige 7
  bronnen worden de standaard.
- **Aanpak:** teksten ingebakken in de exe; voorkeuren in `settings.json`;
  inloggegevens blijven in de ini; adult-filter op één plek (de catalogus);
  instellingen als overlay in de app.
- **Inloggegevens nooit in de log** (gemaskeerd).

Door mij ingevulde standaarden (goedgekeurd in het sectie-ontwerp):

- Instellingen openen met toets **`o`** (Opties / Options / Optionen). Het
  hulpscherm (`?`) noemt de toets, en een klik op die regel opent de instellingen.
- Pincode: precies **4 cijfers**. Na **3 opeenvolgende** foute pogingen **30 s**
  wachten (alleen in het geheugen; een juiste code zet de teller op nul).
  Vergeten: resetten door het portal-wachtwoord in te voeren.
- Zaprichting omdraaien geldt ook voor het **scrollwiel**, niet in de gids.
- Taal bij de eerste start: voorgeselecteerd uit de Windows-weergavetaal
  (Nederlands bij NL, Deutsch bij DE, anders English).

## Performance (bindend — zie [[feedback_performance_priority]])

- Geen extra netwerkverzoeken bij het opstarten: `is_adult` zit al in de
  zender- en filmlijsten die de app nu ophaalt.
- `settings.json` wordt één keer gelezen bij de start en alleen geschreven bij
  een wijziging.
- Vertalingen zijn statische teksten: één `match` per tekst, geen bestanden,
  geen parsing, geen allocatie voor teksten zonder invulwaarden.
- Het adult-filter is één ja/nee-controle in de lijsten die de catalogus nu al
  opbouwt (zaplijst, zoek-items, zenderlijst).
- Het instellingenscherm kost niets zolang het dicht is (één `bool`-controle
  per frame).
- Meetpunt: de opstarttijd tot het eerste beeld bij hervatten, vóór en na deel 1
  (logtijdstempels); geen achteruitgang toegestaan.

## Architectuur

Nieuwe modules naast de bestaande in `rust/src/`:

### `i18n.rs` — teksten

- `pub enum Lang { Nl, En, De }` met `code()` (`"nl"`/`"en"`/`"de"`) en
  `from_code()`.
- `pub enum T { … }`: één variant per zichtbare tekst.
  `pub fn t(lang: Lang, key: T) -> &'static str` met per taal een uitputtende
  `match`. Ontbreekt een vertaling, dan bouwt de app niet.
- Teksten met invulwaarden zijn functies die alle waarden als parameter krijgen
  en de vertaalde zin met de waarden op de juiste plek teruggeven, bijvoorbeeld
  `fn preset_saved(lang, digit: u8, name: &str) -> String`. Zo mag de woordvolgorde
  per taal verschillen.
- Niet vertaald: zender- en programmanamen (van de portal), logregels (Engels,
  voor diagnose) en de venstertitel "Bongo Player vX.Y.Z".
- Alle bestaande zichtbare teksten gaan erdoor: toasts, hulpscherm, de huidige
  gids, lege toestand, portalscherm, zoeken, favorieten, nieuwskiezer, debug-HUD
  en de foutteksten die op het scherm komen (`CredsError`, `PortalError`,
  catalogusstatus). De lege toestand verwijst naar het instellingenscherm (`o`)
  in plaats van naar "run.bat".

### `settings.rs` — voorkeuren

`%APPDATA%\BongoPlayer\config\settings.json` (het ongebruikte pad `config.json`
in `storage.rs` vervalt):

```json
{
  "language": "nl",
  "swap_zap_keys": false,
  "pin": { "salt": "9f3a…", "hash": "c41d…" },
  "news_rules": [
    { "channel": "NPO 1", "patterns": "*journaal*; !*sport*", "label": "NOS", "skip_secs": 0 }
  ]
}
```

- `language` ontbreekt → eerste start: het taalscherm verschijnt.
- `pin` ontbreekt of is `null` → er is nog geen pincode.
- `news_rules` ontbreekt → de standaardregels gelden. Een lege lijst is een
  bewuste keuze: N toont dan "Geen nieuwsregels ingesteld".
- Onbekende velden worden genegeerd. Een kapot of onleesbaar bestand → alle
  standaarden, plus een waarschuwing in de log; de app gaat gewoon door.
- Wegschrijven gebeurt atomair: eerst naar `settings.json.tmp`, dan hernoemen.

### Inloggegevens

- Blijven in `BongoPlayer.ini` naast de exe, in hetzelfde formaat (draagbaar,
  bestaande installaties blijven werken).
- Het instellingenscherm leest en schrijft dezelfde regel
  (`user:pass@host:port`).
- Splitsen gebeurt op de **laatste** `@`, zodat een wachtwoord een `@` mag
  bevatten. Een gebruikersnaam mag geen `:` bevatten (zoals nu).
- Een `--xtream` met placeholder-waarden telt niet: dan geldt de ini. Een
  `--xtream` met echte waarden gaat, zoals nu, voor de ini, bij elke start. Komt
  de portal in gebruik daaruit, dan zegt het portaldeel van de instellingen dat
  onder de velden ("Deze portal komt uit de opdrachtregel (--xtream); wat je
  hier opslaat geldt pas als je zonder die optie start."); na een Save is de
  opgeslagen portal die in gebruik is en verdwijnt de melding.
- De `run.bat` in de zip vraagt niet meer om inloggegevens: hij start alleen de
  app. De portal wordt bij de eerste start gevraagd (na de taalkeuze) of later
  ingesteld met `o`.
- Wat in het portaldeel of in het portalscherm van de eerste start wordt
  geplakt of getypt, wordt eerst opgeschoond: regeleinden (CR/LF) weg uit alle
  velden, spaties rond server en gebruiker weg, bij de server ook een
  voorafgaande `http://` of `https://` en een afsluitende `/`; het wachtwoord blijft
  zoals getypt. Een antwoord op het inloggen dat geen 2xx is (behalve 401/403:
  onjuist) leest als "Server niet bereikbaar": 503 en 509 zijn de gewone
  drukte-antwoorden van deze portal.

### Catalogus en portal

- `LiveChannel`, `Movie` en `Series` krijgen `is_adult: bool`. Het inlezen is
  tolerant: `"1"`, `1` en `true` betekenen ja; ontbreekt het veld, dan nee.
- Adult = `is_adult`, en niets anders. Een naamregel (de naam begint met `XXX`)
  hoorde er eerst ook bij; die is op besluit van de gebruiker van 2026-10-12
  vervallen: hij verborg ook de gewone film *xXx* (2002) en de vervolgen zolang
  de app vergrendeld was, en de vlag is op deze portal betrouwbaar (197 van de
  198 adult-zenders en alle 3.937 adult-films hebben hem).
- `CatalogStore` krijgt `set_adult_unlocked(bool)` (alleen in het geheugen,
  standaard `false`). Alle lijsten die de app gebruikt (`zap_channels`,
  `live_channels`, `search_items`/`search`, het opzoeken van een zender of film
  op id) laten adult weg zolang die `false` is. Eén plek, dus niemand kan het
  filter vergeten.

### `settings_ui.rs` — het scherm

- Een egui-overlay zoals hulp en favorieten. Zolang het open is, krijgt het alle
  invoer (net als het zoekvak).
- Het tekent zichzelf en geeft acties terug aan de app (taal gewijzigd, portal
  opslaan, ontgrendelen, vergrendelen, nieuwsregels gewijzigd, …). `app.rs`
  voert ze uit, zodat `app.rs` niet verder groeit.

### `news.rs` — nieuwsregels (pure logica)

- `NewsRule { channel, patterns, label, skip_secs }` en `default_rules()`.
- `fn title_matches(patterns: &str, title: &str) -> bool`, met `*` = willekeurige
  tekst, hoofdletterongevoelig. Een titel komt mee als hij minstens één gewoon
  patroon raakt en geen enkel `!`-patroon. Leeg of alleen `*` = elk programma.
- De nieuwskiezer in `app.rs` vervangt zijn ingebakken lijst door de regels uit
  de instellingen. Het ophalen, maximaal 3 per regel en 10 in totaal, nieuwste
  eerst, start + `skip_secs` en het label "[TERUG]" blijven zoals nu.

## Gedrag

### Eerste start

1. Ontbreekt `language`, dan verschijnt eerst een taalscherm met drie grote
   knoppen: Nederlands / English / Deutsch. De Windows-weergavetaal is
   voorgeselecteerd; Enter of een klik kiest.
2. Ontbreken de inloggegevens, dan volgt het portalscherm (vertaald). **Opslaan**
   test eerst de verbinding en slaat alleen op als die slaagt.
3. Wie van 0.5.x komt, heeft wel inloggegevens maar geen `settings.json`. Die
   ziet het taalscherm één keer en gaat dan verder zoals altijd.

### Instellingenscherm (`o`)

`o` opent en sluit het scherm in de kijkstand (niet in de gids of het zoekvak);
Esc sluit ook. Muis en toetsenbord werken allebei (Tab, Enter, Esc). Elke
wijziging geldt meteen, behalve de portal.

1. **Taal:** Nederlands / English / Deutsch. Alle teksten wisselen direct, ook
   in het scherm zelf.
2. **Portal:**
   - Velden voor server (host:poort), gebruiker en wachtwoord (gemaskeerd),
     ingevuld vanuit de ini.
   - **Verbinding testen** vraagt bij de portal `player_api.php` op met deze
     gegevens en meldt een van deze uitkomsten:
     - "Verbonden: N zenders"
     - "Gebruiker of wachtwoord onjuist" (`user_info.auth` ≠ 1)
     - "Server niet bereikbaar" (netwerkfout of time-out)
     - "Geen geldige portal" (onverwacht antwoord)
   - **Opslaan** test eerst. Slaagt de test, dan schrijft het de ini en bouwt het
     portal en catalogus opnieuw op, zoals het huidige portalscherm doet. Mislukt
     de test, dan blijft alles zoals het was en blijft de melding in beeld.
3. **Bediening:** "Zaprichting omdraaien (pijl omhoog = volgende zender)". Draait
   pijltjes en scrollwiel in de kijkstand om; de gids verandert niet. Het
   hulpscherm toont de richting zoals ingesteld.
4. **Adult-inhoud:**
   - Vergrendeld: status "Verborgen" en de knop **Ontgrendelen**.
     - Is er nog geen pincode, dan kies je eerst een code: 4 cijfers, 2× invoeren
       en gelijk.
     - Anders voer je de pincode in. Juist → ontgrendeld tot afsluiten.
       3× fout → 30 s geen invoer.
   - Ontgrendeld: status "Zichtbaar tot afsluiten" en de knop **Vergrendelen**.
   - **Pincode wijzigen** vraagt de oude code en daarna de nieuwe 2×.
   - **Pincode vergeten?** vraagt het portal-wachtwoord (zoals in de ini). Juist →
     de pincode is gewist; bij de volgende keer ontgrendelen kies je een nieuwe.
5. **Nieuws (N):**
   - Een lijst regels met de kolommen zender, titelpatroon(en), label en
     "begin later (s)".
   - Achter elke zender staat of hij gevonden is ("NL | NPO 1" of "niet
     gevonden").
   - Knoppen **Regel toevoegen**, **Verwijderen** en **Standaard herstellen**.

### Adult vergrendeld (bij elke start)

- **Zappen:** de zaplijst slaat adult-zenders over.
- **Zoeken:** geen adult-zenders, -films of -series.
- **Favorieten en sneltoetsen:** een verwijzing naar adult gedraagt zich als leeg.
  De favoriet staat niet in de lijst; de sneltoets meldt "sneltoets 5 is leeg".
  Er verschijnt geen pincodevraag, want die zou verraden dat er iets is. Opslaan
  als favoriet of sneltoets kan alleen als de app ontgrendeld is. Zolang de
  catalogus nog niet geladen is, toont de favorietenlijst niets maar "catalogus
  laden...": alleen de catalogus weet welke favorieten adult zijn.
- **Hervatten bij de start:** wijst `last_watched` naar adult (een live- of
  film-URL met een adult-id), dan speelt de app niets af en toont de lege
  toestand; zegt de geladen catalogus het, dan onthoudt het bestand het. Stond
  het bestand op niet-adult en zegt de catalogus pas na het laden dat de
  hervatte stream adult is, dan stopt die zoals bij Vergrendelen (en het
  bestand onthoudt het). Een URL van de opdrachtregel of een eigen keuze blijft
  spelen; het bestand krijgt wel het woord van de catalogus.
- **Nieuwsregels:** een regel waarvan de zender adult is, wordt overgeslagen.
- **Vergrendelen terwijl er adult speelt:** het afspelen stopt (nieuw commando
  `Cmd::Stop`) en de melding "Vergrendeld" verschijnt.

### Nieuws: standaardregels (= het huidige gedrag)

| Zender | Patronen | Label | Begin later (s) |
|---|---|---|---|
| NPO 1 | `*journaal*; !*sport*; !*jeugd*; !*regio*; !*makkelijke taal*; !*gebaren*` | NOS | 0 |
| NPO 2 | idem | NOS | 0 |
| RTL 4 | `*rtl nieuws*` | RTL | 30 |
| CNN | `*` | CNN | 0 |
| BBC News | `*` | BBC | 0 |
| VRT 1 | `*journaal*` | VRT | 0 |
| Omroep Zeeland | `*nieuws*` | ZLD | 0 |

Een zender wordt gevonden zoals nu: eerst een exacte genormaliseerde naam, dan
een deel ervan, met voorkeur voor de terugkijk-versie. Een test bewijst dat deze
regels dezelfde programma's opleveren als de huidige code.

## Foutafhandeling

- `settings.json` kapot of onleesbaar → standaarden, een logwaarschuwing, en de
  app werkt door. Release-builds stoppen bij elke paniek (`panic = "abort"`),
  dus geen `unwrap` op invoer van buiten.
- Wegschrijven mislukt (schijf vol, rechten) → de wijziging geldt tot afsluiten,
  met de melding "Instellingen konden niet worden opgeslagen".
- De verbindingstest heeft een time-out van 10 s en blokkeert de interface niet:
  hij draait op de tokio-runtime met een "bezig…"-status, zoals het huidige
  portalscherm de catalogus laadt.

## Beveiliging

- Pincode: SHA-256 over salt + pincode, met een salt van 16 willekeurige bytes
  per pincode. De pincode zelf wordt nergens opgeslagen of gelogd.
- De grens is eerlijk benoemd: de pincode beschermt tegen per ongeluk kijken
  door gezinsleden, niet tegen iemand die bestanden kan bewerken of wissen. Wie
  `settings.json` verwijdert, wist ook de pincode en de taalkeuze.
- Het instellingenscherm zelf staat niet achter de pincode. Alleen
  adult-inhoud is beschermd.
- **Inloggegevens nooit in de log:** één functie maskeert gebruiker en wachtwoord
  in portal-URL's (`/live|movie|series|timeshift/<u>/<p>/`, `username=`,
  `password=`). Die geldt voor elke logregel met een URL: hervatten, `loadfile`,
  en de mpv-regels die de app doorlogt.
- `last_watched.json` blijft de URL bewaren (zelfde beschermingsniveau als de
  ini, in het eigen gebruikersprofiel).

## Testen

Test-first (TDD), zoals in de rest van het project.

- **i18n:** de volledigheid controleert de compiler. Een unit-test per functie
  met invulwaarden (juiste waarden, alle drie de talen).
- **settings:** opslaan en laden geeft hetzelfde terug; een ontbrekend bestand
  geeft de standaarden; een kapot bestand geeft standaarden zonder paniek;
  onbekende velden worden genegeerd; atomair schrijven.
- **pincode:** instellen en controleren; 3 foute pogingen geven 30 s blokkade (met
  een nagebootste klok); resetten met het juiste en het verkeerde
  portal-wachtwoord.
- **news:** `*`, `!`, hoofdletters, leeg patroon. Plus de standaardregels op een
  set echte titels: "NOS Journaal" ja, "NOS Jeugdjournaal" nee, "NOS Journaal in
  Makkelijke Taal" nee, "NOS Sportjournaal" nee, "RTL Nieuws" ja.
- **inloggegevens:** splitsen op de laatste `@`; placeholder-`--xtream` valt terug
  op de ini; maskeren in logregels (alle URL-soorten).
- **adult:** `is_adult` inlezen (string, getal, ontbrekend) voor live, film en
  serie; catalogusfilter vergrendeld en ontgrendeld voor zaplijst, zoeken en
  opzoeken op id; sneltoets en favoriet als leeg; hervatten overgeslagen.
- **end-to-end met de nep-portal:**
  - taalscherm bij de eerste start, met de juiste voorselectie
  - instellingen openen met `o` en de zaprichting omdraaien
  - adult verborgen in zappen en zoeken; zichtbaar na de pincode; weer verborgen
    na een herstart
  - een nieuwsregel wijzigen en het effect op N
- **handmatig:** nieuwe stappen in `docs/VALIDATION.md`.

## Buiten scope

- De nieuwe gids (deel 2, eigen spec).
- M3U-afspeellijsten als bron.
- Live tv pauzeren; series afspelen (de seriekiezer bestaat nog niet).
- Meer talen dan NL/EN/DE.
- Het instellingenscherm zelf achter een pincode.
