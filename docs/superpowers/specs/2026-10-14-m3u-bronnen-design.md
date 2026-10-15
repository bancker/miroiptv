# Bronnen: M3U-lijsten naast Xtream (ronde 1)

**Datum:** 14 oktober 2026
**Status:** ontwerp, goedgekeurd in drie delen in het gesprek van 14 okt 2026. Het document zelf moet de gebruiker nog lezen.
**Basis:** Bongo Player 0.6.1 (`main`, 7885d4f)

## 1. Doel

Bongo Player kent nu één soort bron: een Xtream-portaal (`--xtream user:pass@host:port`, of het portaal in
`BongoPlayer.ini`). Daarnaast komen M3U-lijsten, voor twee groepen gebruikers:

- **De gebruiker zelf.** Hij wil gratis zenders, zoals de openbare lijst van iptv-org
  (`https://iptv-org.github.io/iptv/index.m3u`), naast zijn eigen portaal.
- **Kopers van het latere product.** Veel aanbieders leveren alleen een M3U-link. Zonder M3U kunnen
  die klanten Bongo niet gebruiken.

De lijst van iptv-org, opgehaald op 13 oktober 2026, ziet er zo uit:

| Kenmerk | Waarde |
|---|---|
| Omvang | 2,55 MB, 11.262 zenders |
| Gegevens per zender | `#EXTINF` met `tvg-id`, `tvg-logo` en `group-title` (soms meer groepen met `;`), gevolgd door de naam en daarna de URL |
| Soort stream | 97% HLS (`.m3u8`): 8.906 https en 2.347 http. Een handvol is rtmp, srt of mmsh |
| Extra headers | 1.511 zenders hebben `#EXTVLCOPT`-regels voor de User-Agent of Referer |
| EPG | De kop wijst met `x-tvg-url` naar één XMLTV-bestand (`guide.xml.gz`) |
| Groepen | 183 groepen (General, News, Movies, Kids, …), geen volwassenengroepen |

## 2. Beslissingen van de gebruiker

| Vraag | Keuze |
|---|---|
| Waarvoor? | Beide: gratis zenders voor hemzelf en M3U-klanten van het product |
| Hoe gaan bronnen samen? | **Eén bron tegelijk**, te kiezen in de instellingen, zoals in IPTV Smarters en MyTVOnline |
| Wat komt eerst? | **Ronde 1 is alleen de zenders.** De gids (XMLTV) komt in ronde 2, met een eigen ontwerp |
| Pincode bij M3U? | **Niets vergrendelen.** Alleen Xtream-portalen kennen volwassen inhoud (vlag `is_adult`) |
| Aanpak | **A: M3U als tweede soort portaal.** Het levert dezelfde zenderlijst aan, zonder films, series en gids |

## 3. Ontwerp

### 3.1 Bronnen en instellingen

**Het paneel**
- Het paneel "Portaal" in de instellingen (`o`) heet voortaan **"Bronnen"**. Het toont een lijst met
  namen, met de actieve bron gemarkeerd.
- Je kunt een bron **toevoegen, wijzigen, verwijderen en actief maken**.

**Soorten bronnen**

| Soort | Velden |
|---|---|
| Xtream | server, gebruiker, wachtwoord, zoals nu (met dezelfde verbindingstest) |
| M3U | een URL (`http`/`https`) of een pad naar een bestand op schijf |

Elke bron heeft ook een naam die de gebruiker zelf kiest.

**Wisselen en starten**
- Een andere bron actief maken laadt de zenderlijst van die bron.
- Bij het opstarten opent de app de bron die als laatste actief was.

**Een M3U-link die eigenlijk een Xtream-portaal is**
- Herkenning: een M3U-adres van de vorm `…/get.php?username=…&password=…`.
- De app meldt dat en stelt voor er een Xtream-bron van te maken. Daarmee krijg je films, series,
  de gids en terugkijken.
- De gebruiker kiest. Zegt hij nee, dan blijft het een M3U-bron.

**Wachtwoorden en logs**
- Een M3U-adres kan een gebruiker en wachtwoord bevatten. Het wordt daarom behandeld als
  inloggegevens:
  - het staat alleen in `BongoPlayer.ini`;
  - het komt nooit in een logregel of foutmelding (`redact.rs` maskeert ook de query van een
    M3U-adres);
  - het komt nooit in een bestandsnaam.

**Eerste start en eerste bron**
- **Eerste start na de update:** het huidige portaal (`portal=…` in de ini) wordt automatisch
  bron 1, met de naam "Portaal" (NL), "Portal" (EN) of "Portal" (DE). Er hoeft niets opnieuw te
  worden ingevoerd.
- **Eerste start zonder bron:** het bestaande startscherm vraagt zoals nu om een Xtream-portaal. Het
  krijgt één regel die zegt dat een M3U-lijst toevoegen kan via de instellingen (`o` → Bronnen).
  Het formulier zelf verandert niet.

### 3.2 Opslag

**`BongoPlayer.ini`**
- De ini bewaart alle bronnen (naam, soort, adres) en welke bron actief is.
- De bestaande regel `portal=user:pass@host:port` blijft voor de eerste Xtream-bron staan. Zo
  start een terugstap naar 0.6.1 nog steeds op het portaal.
- Wegschrijven gebeurt zoals nu: eerst een tijdelijk bestand, dat daarna het oude vervangt.

**Gegevens per bron**
- Favorieten, presets en de laatst gekeken zender worden per bron bewaard. Een bron krijgt bij het
  aanmaken een korte, vaste id. Die hangt niet af van het adres, zodat favorieten blijven als je het
  adres wijzigt.
- De bestaande `favorites.json`, `presets.json` en `last_watched.json` horen bij bron 1.
- `settings.json` (taal, nieuwsregels, pincode) blijft voor alle bronnen samen.

**Bewaarde lijsten**
- De laatst opgehaalde lijst van een M3U-bron staat in `config\cache\m3u-<bron-id>.m3u`. In de
  bestandsnaam staat geen adres.
- Een verwijderde bron neemt zijn cachebestand en zijn eigen gegevens mee.

**De credentialscan**
- `rust/e2e/check-staged.sh` controleert nu alle gebruikers, wachtwoorden en M3U-adressen uit de ini,
  niet alleen de eerste regel met een `@`.

### 3.3 Laden

**Een M3U-bron met een URL**
- De app toont meteen de bewaarde lijst uit `config\cache\`.
- Daarna haalt hij op de achtergrond de nieuwe lijst op, met dezelfde HTTP-client en User-Agent als
  het portaal en een time-out. Is die anders, dan vervangt hij de oude, en de cache wordt bijgewerkt.
- Opstarten en het hervatten van de laatste zender wachten nooit op de download: hervatten gebruikt
  het adres uit de bewaarde lijst.

**Een M3U-bron met een bestand**
- De app leest het bestand direct, op een eigen thread. Er is geen cache nodig.

**Als het misgaat**
- Mislukt de download, dan blijft de bewaarde lijst staan, met een korte melding: "Zenderlijst niet
  bijgewerkt" (NL, EN, DE).
- Zonder bewaarde lijst toont de app dezelfde lege toestand en melding als bij een onbereikbaar
  portaal.

**Opnieuw ophalen**
- F5 haalt de lijst opnieuw op, zoals nu bij de catalogus van het portaal.
- De gids en andere onderdelen die de catalogusversie volgen, volgen ook de M3U-lijst.

### 3.4 Inlezen

**Algemeen**
- Het inlezen gebeurt op een eigen thread en is tolerant. Het accepteert een BOM, CRLF en een
  ontbrekende `#EXTM3U`-kop.
- **Kapotte regels:**
  - een `#EXTINF` zonder URL wordt overgeslagen;
  - een URL zonder `#EXTINF` wordt een zender met de URL als naam;
  - onbekende `#`-regels worden genegeerd.

**Kop**
- `x-tvg-url`, of `url-tvg`, wordt bewaard voor ronde 2. In ronde 1 doet de app er niets mee.

**`#EXTINF`**
- De naam is de tekst na de laatste komma buiten aanhalingstekens.
- De kenmerken zijn `tvg-id`, `tvg-name`, `tvg-logo` en `group-title`. Een groep kan meer namen
  bevatten, gescheiden door `;`.
- Waarden staan tussen dubbele aanhalingstekens. Onbekende kenmerken worden genegeerd.

**`#EXTVLCOPT`**
- `http-user-agent` en `http-referrer` gelden voor de eerstvolgende zender.

### 3.5 De zenderlijst

**Volgorde en zappen**
- De volgorde is die van het bestand.
- Er worden geen varianten samengevoegd. De samenvoeging op basis van kwaliteit in `catalog.rs` is
  gemaakt voor de naamgeving van het Xtream-portaal en blijft alleen voor Xtream.
- ↑/↓ zapt door de lijst, met `zap.rs` voor het tempo, zoals nu.
- Zoeken (`f`), favorieten (`*`, Shift+F), presets (cijfers) en het nieuws (`n`, op naam) werken zoals
  nu over de zenders van de actieve bron.

**Vaste sleutels**
- Elke zender krijgt een vaste sleutel:
  - de `tvg-id`, als die er is en uniek is in de lijst;
  - anders de naam;
  - een volgende zender met dezelfde sleutel krijgt "#2", "#3", enzovoort, in de volgorde van het
    bestand.
- De `stream_id` (i64) die de rest van de app gebruikt, is een stabiele hash van die sleutel.
  Favorieten en presets blijven daardoor staan als de aanbieder de volgorde of de adressen van zijn
  lijst verandert.
- Voor die hash gebruik je een vaste functie: de eerste 8 bytes van SHA-256, via `ring`, dat al een
  afhankelijkheid is, teruggebracht tot een positieve i64. Gebruik niet `DefaultHasher`: die belooft
  niet dezelfde uitkomst over Rust-versies heen.

**De gids**
- `GuideChannel`s zijn er voor een M3U-bron niet: de lijst kent geen terugkijken.
- `g` toont een melding in NL, EN en DE: "Voor deze bron is er nog geen gids".

### 3.6 Afspelen

**Een M3U-zender**
- De zender gaat rechtstreeks naar mpv, niet via de relay. De relay is er voor het Xtream-portaal
  (één sessie per account, de weigeringen 407/503/509) en blijft alleen voor Xtream.
- Heeft de zender een User-Agent of Referer, dan geeft de app die alleen voor die ene load aan mpv
  mee, als opties per bestand. De volgende load gebruikt weer de standaard.

**Bewaking en fouten**
- De bewaking blijft gewoon werken: de controle op een bevroren beeld (`playback_health.rs`, zoals
  bij live) en het automatisch herladen bij een onverwacht einde (`ReloadBudget`).
- **Een dode zender** (404, time-out, of alleen in het eigen land te zien):
  - de bestaande foutmelding met de naam van de zender;
  - de app blijft op die zender staan en springt niet vanzelf door.
- RTMP, SRT en MMS worden doorgegeven aan mpv en werken als de meegeleverde libmpv ze kent.

### 3.7 Wat een M3U-bron in ronde 1 niet heeft

- **Films en series als aparte soort.** In een M3U zijn dat gewoon zenders in hun groep.
- **De gids, de EPG in de zapmelding en terugkijken.** De zapmelding toont alleen de naam.
- **Logo's.** Die staan nu alleen in de gids en komen in ronde 2.
- **De pincode.** Die doet niets bij een M3U-bron (beslissing 2).
- **Meer bronnen tegelijk** (beslissing 2).

### 3.8 Prestaties

Snelheid gaat voor alles. De grenzen:

| Onderwerp | Grens |
|---|---|
| Opstarten en zappen | Niet trager dan 0.6.1. Dat wordt gemeten met de startmeting tegen 0.6.1, met een Xtream-bron en met een actieve M3U-bron van ongeveer 11.000 zenders |
| Inlezen | 11.000 zenders in minder dan 100 ms (release), op een eigen thread. De UI-thread doet alleen het overzetten |
| Geheugen | Minder dan 10 MB extra voor een lijst van 11.000 zenders. Daarvan worden alleen de velden bewaard die ronde 1 gebruikt, plus `tvg-id`, `tvg-logo` en de `x-tvg-url` voor ronde 2 |
| Per frame | Geen werk dat meegroeit met de lijst. Zoeken blijft gebonden aan de toetsaanslagen, zoals nu |

## 4. Foutafhandeling

| Situatie | Wat de gebruiker ziet |
|---|---|
| Download mislukt, wel een bewaarde lijst | De oude lijst, plus de melding "Zenderlijst niet bijgewerkt" |
| Download mislukt, geen bewaarde lijst | De lege toestand en de melding, zoals bij een onbereikbaar portaal |
| Geen M3U (bijvoorbeeld HTML of een lege lijst) | De melding "Dit is geen zenderlijst", zonder de lijst te vervangen |
| Bestand niet gevonden of niet leesbaar | Een melding met het pad, maar zonder inloggegevens |
| Dode zender | De bestaande foutmelding; de app blijft op de zender staan |
| Ongeldige invoer in de instellingen | Opslaan kan niet, met een melding bij het veld. Dat geldt voor een lege naam en een adres dat geen http(s)-URL of bestaand bestand is |

## 5. Privacy en beveiliging

- Inloggegevens, ook die in een M3U-adres, staan alleen in `BongoPlayer.ini`. Ze komen nooit in
  logs, foutmeldingen, cachebestandsnamen of de zapmelding.
- De cache bevat de lijst zoals de aanbieder hem levert. Een M3U van een betaalde aanbieder kan in
  de stream-URL's zelf gebruikersnamen bevatten. Daarom staat de cache onder `config\`, net als de
  andere gegevens van de gebruiker. Hij komt niet naast de exe en gaat nooit mee in een zip.

## 6. Testen

**Unit-tests**
- Het inlezen:
  - kenmerken, ook meer groepen in één veld;
  - de headers via `#EXTVLCOPT`;
  - een BOM, CRLF, een ontbrekende kop en kapotte regels;
  - de `x-tvg-url`.
- De sleutels: een unieke `tvg-id`, en dubbele namen die "#2" krijgen.
- Het herkennen van een Xtream-link.
- De bronnen in de ini: lezen, schrijven, de overgang van `portal=` en het laten staan van die regel.
- `redact` op een M3U-adres met een wachtwoord.

De testlijst is zelfgemaakt in het formaat van iptv-org, zonder echte adressen.

**App-tests** (`app::tests::Harness`, zonder venster)
- Wisselen van bron, waarbij de bewaarde lijst vóór de download verschijnt.
- Een mislukte download, waarbij de oude lijst blijft staan.
- Zappen en zoeken in een M3U-lijst.
- Afspelen:
  - een `LoadUrl` rechtstreeks naar de URL van de zender, niet via de relay;
  - met de User-Agent en Referer van die zender;
  - de volgende load zonder die headers.
- Een dode zender geeft de melding.
- `g` meldt dat er nog geen gids is.
- Favorieten en presets worden per bron bewaard.
- De pincode doet niets bij een M3U-bron.

**Prestatietest**
- 11.000 zenders inlezen binnen de grens uit §3.8. In debug geldt een ruimere grens, met de
  release-meting in het verslag.

**Nepserver en venstertests** (alleen met toestemming van de gebruiker)
- `mock_portal.py` krijgt met `MOCK_M3U=1` een lijst van ongeveer 20 zenders op de eigen
  HLS-segmenten:
  - één zender speelt alleen met de juiste User-Agent;
  - één zender is dood (404).
- Nieuwe scenario's:
  - een M3U-bron toevoegen via `o`;
  - van bron wisselen;
  - zappen en zoeken;
  - de zender met de User-Agent;
  - de dode zender.
- De hele regressieset (nu 101 scenario's) moet daarna weer slagen. Scenario's van deel 1 die Tabs
  tellen in het oude paneel "Portaal", worden aangepast waar het paneel verandert.

**Startmeting** tegen 0.6.1, zie §3.8.

## 7. Documentatie

- **`docs/VALIDATION.md`** krijgt een sectie "Bronnen (M3U)" met de handmatige controles.
- **`CLAUDE.md`**:
  - `portal/m3u.rs` als tweede `Portal`;
  - de relay alleen voor Xtream;
  - opslag per bron;
  - M3U-adressen gelden als inloggegevens.
- **`rust/README.md`**: bronnen en M3U.

## 8. Ronde 2 (buiten dit ontwerp)

- XMLTV via `x-tvg-url` (of een eigen URL): nu en straks in de zapmelding, en de gids (`g`) voor
  M3U-zenders.
- Logo's uit `tvg-logo`.

Ronde 2 krijgt een eigen ontwerp. Het XMLTV-bestand van iptv-org is groot, en een kleine
geheugenvoetafdruk vraagt een eigen aanpak.

## 9. Risico's

| Risico | Gevolg en maatregel |
|---|---|
| Het paneel "Portaal" wordt "Bronnen" | e2e-scenario's van deel 1 tellen Tabs. Het Xtream-formulier blijft zo veel mogelijk gelijk, en het formulier van het startscherm verandert niet: het krijgt alleen een tekstregel. Scenario's worden waar nodig aangepast |
| De LGPL-build van libmpv (het licentieproject) | Kan protocollen als SRT of RTMP laten vallen. Dat is acceptabel: ze zijn zeldzaam en het is "best effort" |
| Willekeurige servers | Ze gedragen zich anders dan het portaal: trage of dode streams, en vaak een eigen User-Agent. Daarom zijn er de headers per zender en de foutmelding zonder doorspringen |
| Zappen door 11.000 zenders | Kan traag aanvoelen. `zap.rs` regelt het tempo al, en favorieten en presets zijn de snelle weg |

## 10. Uitwerking in het bouwplan

Het bouwplan (`docs/superpowers/plans/2026-10-14-m3u-bronnen.md`, "Rulings") legt vast wat dit ontwerp
openliet. Kort:

- `o` werkt niet in het startscherm (typen mag nooit een sneltoets zijn). Daarom is de nieuwe regel een
  link naar Bronnen met een nieuwe M3U-lijst klaar.
- De ini: `portal=` (kopie van de eerste Xtream-bron), `source=<id>;<soort>;<naam>;<adres>` en
  `active=<id>`. Eén portaal met de standaardnaam blijft precies de regel van 0.6.1.
- Een bron zonder eigen naam heet "Portaal" of "M3U-lijst" in de taal van de app.
- Bron 1 houdt de bestanden van 0.6.1; andere bronnen hebben bestanden met hun id. `--xtream` gebruikt die
  van bron 1. Die bestanden zijn van het portaal, dus een M3U-lijst wordt nooit bron 1: een nieuwe lijst
  krijgt 1 + het hoogste id, minstens 2, en een nieuw portaal krijgt 1 als er nog geen portaal is en 1 vrij
  is. Een oudere ini met een lijst als bron 1 wordt gelezen zoals hij is.
- Van bron wisselen werkt als een start op die bron: wat speelt stopt, de laatste zender van die bron
  hervat. Een nieuwe bron gaat alleen meteen in gebruik als er geen bruikbare bron in gebruik is.
- Verwijderen vraagt een tweede druk, en de bron die in gebruik is kan niet weg.
- De headers per zender gaan mee in de URL, op de manier van Kodi (`url|User-Agent=…`). mpv krijgt ze als
  opties voor dat ene bestand. Alleen een load van een M3U-bron splitst ze af; elke andere URL gaat
  ongewijzigd naar mpv, zoals in 0.6.1, want een Xtream-wachtwoord kan een `|` bevatten.
- De naam in `#EXTINF` begint na de eerste komma buiten aanhalingstekens, niet na de laatste: een naam mag
  zelf komma's hebben.
- Een download die gelijk is aan de bewaarde lijst verandert niets. Een HTML-pagina of een lijst zonder
  zenders is "geen zenderlijst". Dat is ook een lijst van meer dan 128 MB, gedownload of in een bestand
  (er wordt niet meer van gelezen), en wat in de eerste KB een NUL-byte heeft (een video, een archief).
- De foutmelding van een dode zender noemt de zender, ook bij Xtream.
- Logregels noemen een M3U-bron bij id en host, nooit bij adres.
- De versie gaat in dit plan niet omhoog.

Tijdens het bouwen kwamen er besluiten bij. Waar ze afwijken van de paragrafen hierboven (§3.4: "de laatste
komma", §3.5: "een positieve i64"), gaan ze voor:

- De `stream_id` volgt uit de eerste 8 bytes van de SHA-256 van de sleutel (big-endian), één bit naar rechts
  geschoven (`>> 1`), zodat hij nooit negatief is. Opgeslagen favorieten en presets bevatten deze id's, dus
  de uitkomst mag nooit meer veranderen.
- mpv krijgt de User-Agent als de optie `user-agent` en de Referer als een regel in `http-header-fields`
  (`Referer: …`, een komma als `\,`), allebei alleen voor dat ene bestand. De optie `referrer` is niet
  gebruikt: libmpv zette die na een bestand terug als lege tekst, en elke volgende load stuurde dan een lege
  `Referer:`.
- Een NUL-byte in een argument van een mpv-opdracht (een lijst kan er een bevatten) geeft een fout in plaats
  van een paniek, want de release-build breekt bij een paniek af. De zender krijgt de melding van een dode
  zender.
- Een M3U-zender die mpv weigert, of die met een fout eindigt voordat hij een beeld heeft getoond, laat het
  scherm zwart (de lege toestand). Het beeld van de zender ervoor blijft dus niet bevroren onder de nieuwe
  naam staan. Heeft de zender al gespeeld, dan blijft zijn eigen laatste beeld staan. Xtream houdt het
  gedrag van 0.6.1.
- De vraag of een `…/get.php?username=…&password=…`-link een Xtream-bron moet worden, komt alleen bij een
  `http://`-link: de Xtream-client spreekt alleen gewoon HTTP, dus een `https://`-link zou een bron worden
  die nooit verbindt. Een `https://`-link blijft een M3U-lijst, zonder vraag. De vraag hoort bij het adres:
  wie het adres wijzigt, krijgt hem opnieuw, en een bewaarde lijst met hetzelfde adres krijgt hem niet nog
  eens als alleen de naam verandert.
- Een naam die gelijk is aan de standaardnaam van zijn soort, in welke van de drie talen ook, geldt niet als
  eigen naam en wordt leeg bewaard. Hij verschijnt dan in de taal die op dat moment is gekozen.
- Een `;` in een naam wordt bij het bewaren een komma (`;` scheidt de velden van de `source=`-regel). Hij
  valt niet weg. Regeleinden vallen wel weg.
- De ini wordt bij elke wijziging van de bronnen eerst geschreven. Mislukt dat, ook bij van bron wisselen,
  dan verandert er niets en noemt de melding de schrijffout. Zo is wat draait altijd wat de volgende start
  opent.
- Een relatief pad naar een lijstbestand wordt als absoluut pad bewaard (`std::path::absolute`): de volgende
  start draait misschien in een andere map.
- `redact()` maskeert ook `user:pass@` in elke URL, de query van een lijstadres (`.m3u`, `.m3u8`,
  `get.php`, in elke hoofdletter) en de korte Xtream-vorm van een stream-URL
  (`host/<gebruiker>/<wachtwoord>/<cijfers>[.ext]`), zoals betaalde lijsten die gebruiken. Een token in de
  query van een andere URL (`…/1.ts?token=…`) wordt niet gemaskeerd. Elke logregel, foutmelding of toast die
  een adres of een stream-URL noemt, gaat door `redact()` of noemt alleen id en host.
- Een zender zonder naam in de lijst heet naar zijn `tvg-name`, anders naar zijn URL met het account
  gemaskeerd. Die naam komt in meldingen, zoeken en favorieten.
- Het pad van een lokaal bestand is geen inlogkenmerk: de lege toestand en het log mogen het noemen.
- Het resultaat van een verbindingstest of van een Opslaan staat alleen onder het formulier dat erom vroeg.
  Komt de uitslag van een Opslaan nadat de gebruiker naar een ander formulier is gegaan, dan blijft dat
  formulier staan en is de bron toch bewaard.
- Bronnen houdt de plek van het oude Portaal (tweede in de lijst). Een uitgeschakelde knop is geen Tab-stop.
  `o`, ↓, Enter zet de cursor in het veld Server (bij een lijst: Adres) van de bron in gebruik, zodat de
  Tab-tellingen naar Test en Opslaan gelijk blijven. Na een antwoord van de app (opslaan, wisselen,
  verwijderen) staat de cursor in de naam van het formulier dat dan verschijnt.
