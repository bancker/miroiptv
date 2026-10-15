# Thuisbioscoop: series en films (ontwerp A+B)

- **Datum:** 2026-10-15
- **Branch:** `feat/thuisbioscoop`
- **Bouwt voort op:** `main`, na de M3U-bronnen. De zenderlijst wordt eerst afgebouwd en de bouw van dit ontwerp begint daarna.
- **Schetsen:** in `2026-10-15-thuisbioscoop-schetsen/`. Ze zijn gemaakt in de brainstorm-companion; open ze daar of in een browser. Buiten de companion ontbreekt de kaderopmaak, maar de schermen zelf staan er volledig.

## 1. Doel

De gebruiker vroeg een Netflix-achtige manier om series te kijken die op zijn Xtream-portal (Pandora) staan: "zoeken op serienaam. vinden. selecteren seizoen. selecteren aflevering". Daarna kwamen film-info en een waarderingssysteem erbij, en de visie: "Het moet echt een thuis bioscoop onderdeel van de app worden. Pathe thuis, Netflix, Disney+ achtig. Geweldige kwaliteit. Makkelijk en vloeiend te bedienen. Intuïtief is het sleutelwoord bij het zoeken, filteren, beoordelen of je gaat kijken en selecteren gaat super handig, vlot en makkelijk."

De portal heeft 10.589 series en 47.188 films (app-log van 2026-10-15). Vandaag kan de app geen serie afspelen: een serie kiezen in `f` geeft "series afspelen kan nog niet" (`T::SeriesNotYet`).

**Geslaagd is het als:**
1. **Zoeken tot kijken in een handvol toetsen.** Je zoekt een serie op naam en filtert op genre, jaar, waardering of taal. Je kiest een seizoen en een aflevering, en kijkt. Voor een film gaat het net zo.
2. **Verder kijken weet waar je was,** per aflevering en per film. Aan het eind van een aflevering volgt de volgende vanzelf.
3. **Je kunt beslissen of je gaat kijken:** cover, waardering met bron, jaar, duur, beschrijving, cast en een trailer.
4. **Thuis voelt als een streamingdienst:** een uitgelichte titel, rijen covers, Verder kijken, Mijn lijst en je eigen 👍/👎.
5. **Alles haalt de snelheidsnormen van §5.** `f` en zappen worden niet trager, en het zoeken van `h` is aantoonbaar minstens zo snel als `f` nu.

## 2. Wat de gebruiker koos

| Onderwerp | Keuze | Schets |
|---|---|---|
| Opknippen | A (series kijken) en B (info en waardering) samen in dit ontwerp. C (OMDb en TMDB als aanvulling, met bronvermelding) komt daarna als eigen ontwerp. | – |
| Ingang | Toets `h` opent de thuisbioscoop. **`f` blijft precies zoals hij nu is** ("ik wil 100% zeker zijn dat het even snel is als optie f nu"). | – |
| Thuis | **A, Netflix-stijl:** een uitgelichte titel met rijen eronder. | `home-layout.html`, opzet A |
| Seriepagina | **2, een volledige pagina in twee kolommen.** | `series-page.html`, opzet 2 |
| Zoeken en filteren | **Z2, een filterkolom links met aantallen,** en rechts het raster. | `search-filter.html`, opzet Z2 |
| Filmpagina | **F2, rechts een groot raster "Meer zoals dit".** | `film-page.html`, opzet F2 |
| Bediening tijdens het kijken | Het voorstel, akkoord ("bediening ok"). | `film-page.html`, onderaan |
| Volgende aflevering | Een kaart met een aftelling van 10 s. | `series-page.html`, onderaan |
| Trailer | Ja, via yt-dlp. | – |
| Tv tijdens de thuisbioscoop | Speelt door, onzichtbaar en zonder geluid. Esc gaat terug zonder opnieuw te laden. | – |
| ＋ / 👍 / 👎 en de rijen | Akkoord met het voorstel (§3.8 en §3.2). | – |
| Techniek | **Aanpak A:** alles in egui, met een titel-index in het geheugen. Details en covers op aanvraag, met een cache op schijf. | – |
| Bouwen | Drie fasen, elk met een eigen plan (§9). | – |

## 3. Schermen en bediening

Alle tekst die de gebruiker ziet staat in `i18n.rs`, in NL, EN en DE. Logregels zijn Engels. De muis werkt overal mee: klikken is Enter, scrollen beweegt de rijen. Dit hoofdstuk beschrijft het toetsenbord.

### 3.1 Openen en sluiten

- **`h` opent de thuisbioscoop.**
  - Alleen bij een Xtream-bron. Bij een M3U-bron geeft `h` een melding: "De thuisbioscoop werkt met een Xtream-bron".
  - Hij opent op het laatst gebruikte tabblad, de eerste keer op Thuis.
- **Wat de tv doet zolang hij open is:**
  - mpv krijgt `mute=yes`, en de app slaat het uploaden van tv-beelden naar de textuur over, omdat de thuisbioscoop het hele venster bedekt.
  - mpv blijft de stream binnenhalen, zodat de terugkeer direct is.
- **Esc gaat één stap terug:**
  - van een pagina naar waar je vandaan kwam;
  - van een tabblad naar Thuis;
  - van Thuis sluit de thuisbioscoop, met `mute=no` en het tv-beeld weer zichtbaar.
- **`h` sluit de thuisbioscoop direct,** behalve als je in het zoekveld typt; daar is `h` gewoon een letter.
- **Een film of aflevering starten stopt de tv,** omdat de portal één sessie per account toestaat. Sluit je de thuisbioscoop daarna, dan laadt de app de laatst gekeken zender opnieuw, als een gewone zap.
- **Bovenaan staan de tabbladen** Thuis · Series · Films · Mijn lijst · 🔍 Zoeken.
  - ↑ vanaf de bovenste rij gaat naar de tabbalk, ←/→ kiest een tabblad en Enter of ↓ opent het.
- **Een letter of cijfer** die je op Thuis, Series, Films of Mijn lijst typt, opent Zoeken met dat teken al in het veld. De uitzondering is `h`, die sluit.
- **Series en Films** zijn Thuis-achtige rijen voor alleen series of alleen films: Nieuw, Best gewaardeerd en de categorieën van die soort.
- **Mijn lijst** is een raster van je lijst, het laatst toegevoegd eerst.
- **De help (`?`)** krijgt `h` en de toetsen van de thuisbioscoop erbij.

### 3.2 Thuis (opzet A)

**Uitgelicht (bovenaan):**
- Een titel met een grote achtergrond, de titel in het display-lettertype, ★ met bron, jaar, leeftijd, genre, aantal seizoenen of duur, twee regels beschrijving en twee knoppen:
  - **▶ Afspelen.** Bij een serie is dit "Verder kijken · S·A" als er voortgang is, anders S1 A1. Bij een film start hij de film.
  - **ⓘ Meer info.** Opent de pagina van de titel.
- **De keuze:** de titels die de laatste 30 dagen zijn toegevoegd, met ★ 7,0 of hoger en een cover of achtergrond. Uit de tien best gewaardeerde daarvan komt bij elke opening de volgende aan de beurt.
  - Die beurt staat alleen in het geheugen. De eerste opening na het starten toont de best gewaardeerde.
  - Zijn er geen, dan de best gewaardeerde titel van alles.

**De rijen, van boven naar beneden.** Een rij zonder inhoud verschijnt niet. Een rij heeft hooguit 40 tegels.

| # | Rij | Inhoud |
|---|---|---|
| 1 | **Verder kijken** | Wat tussen 1 minuut en 90 % gekeken is. Ook een serie waarvan de laatste gekeken aflevering af is, als er een volgende aflevering is: die wordt de tegel. Het laatst gekeken eerst. Liggende tegels met de still, "S2 · A5 · nog 23 min" en een voortgangsbalk. |
| 2 | **Mijn lijst** | Het laatst toegevoegd eerst. |
| 3 | **Nieuw toegevoegd** | Series en films door elkaar, op de toevoegdatum van de portal, de laatste 30 dagen. Het nieuwste eerst, met een label "NIEUW" voor de laatste 7 dagen. |
| 4 | **Omdat je *X* leuk vond** | Alleen als er een 👍 is. X is het meest recente 👍. Titels uit dezelfde genres of categorie, die je nog niet zag en niet 👎 gaf, best gewaardeerd eerst. Hooguit twee van deze rijen, voor de twee meest recente 👍. |
| 5 | **Best gewaardeerd: series** | ★ hoog naar laag. Titels zonder waardering doen niet mee. Bij gelijke ★ gaat de nieuwste voor. |
| 6 | **Best gewaardeerd: films** | Net zo. |
| 7 | **Categorieën van de portal** | De 8 categorieën met de meeste titels, van films en series samen, elk als eigen rij, het nieuwste eerst. De tabbladen Series en Films nemen de 8 grootste van hun eigen soort. |

**Tegels en focus:**
- Staande covers (2:3) met de titel en ★. Verder kijken heeft liggende tegels (16:9).
- De gekozen tegel krijgt een witte rand en vergroot tot 107 % met een korte animatie.
- ↑/↓ wisselt van rij, ←/→ loopt door een rij, Enter opent de pagina van de titel en ▶ (Spatie) speelt direct.
- **👎 telt overal mee:** een titel met 👎 verschijnt in geen enkele rij en niet als Uitgelicht.

### 3.3 Zoeken (opzet Z2)

**Links (±21 % van de breedte):**
- Het zoekveld, en daaronder de filtergroepen met aantallen:
  - **Soort:** Alles, Series, Films;
  - **Genre:** de vaste lijst van §4.2, met per genre het aantal titels;
  - **Jaar:** 2020 en later, 2010–2019, 2000–2009, eerder;
  - **Waardering:** ★ 8 en hoger, ★ 7 en hoger;
  - **Taal:** Nederlands, Engels, andere.
- De aantallen gelden voor de huidige zoekterm plus de andere filters, dus ze laten zien wat een filter oplevert.

**Rechts:**
- Bovenaan het aantal titels, de actieve filters als gele labels met ✕, en Sorteer: Relevantie, Best gewaardeerd, Nieuwst of Titel (A–Z).
- Daaronder het raster met covers. Elke tegel heeft een label SERIE of FILM, de titel, ★ en het jaar.

**Toetsen:**
- **Typen gaat altijd naar het zoekveld,** waar de focus ook staat. ⌫ werkt in het veld.
- **Enter in het zoekveld** opent de pagina van de bovenste treffer.
- **↓ vanuit het veld** gaat naar de filters.
- **In de filters:** ↑/↓ beweegt, Spatie of Enter zet een filter aan of uit, en → gaat naar het raster.
- **In het raster:** ←/→/↑/↓ beweegt, Enter opent een pagina, en ← in de eerste kolom gaat terug naar de filters.

**Zoeken op titel:**
- Elk woord moet in de titel voorkomen. Hoofdletters en accenten tellen niet: "pokemon" vindt "Pokémon".
- Bij "Relevantie" gaat een titel die met de zoekterm begint voor een titel die hem alleen bevat. Daarna komt ★.
- Live-zenders staan niet in deze zoekpagina; die blijven bij `f`.

### 3.4 Seriepagina (opzet 2)

**De achtergrond:** de achtergrond van de serie, gedimd, over het hele scherm.

**Links: alles om te beslissen.**
- **Titel en kenmerken:** de titel in het display-lettertype, ★ met bron (PANDORA), jaar of jaren, leeftijd, het aantal seizoenen en het genre.
- **De knoppen:**
  - **▶ Verder kijken · S·A**, of "▶ Afspelen · S1 A1" als je nog niets zag, of "↺ Opnieuw kijken" als alles gezien is;
  - **↺ Vanaf het begin**, alleen als er voortgang is;
  - **Trailer**, alleen als de portal een YouTube-code geeft;
  - **＋** (Mijn lijst), **👍** en **👎**.
- **De tekst:** de beschrijving en daaronder "Met: …" en "Genre: …".

**Rechts: seizoenen en afleveringen.**
- **De seizoenen** staan als tabs. Seizoen 0 heet "Extra's". Een serie zonder seizoenindeling krijgt één tab "Afleveringen".
- **Per aflevering:**
  - de still (16:9), het nummer, de titel, de duur en twee regels beschrijving;
  - ✓ als hij gezien is, een gele balk als hij half gezien is, en het label "NOG 23 MIN";
  - "NIEUW" als hij de laatste 7 dagen is toegevoegd.

**Focus en toetsen:**
- De focus start op de hoofdknop. → of Tab gaat naar de lijst, bij de huidige aflevering. Tab gaat terug naar de knoppen.
- In de lijst wisselt ←/→ van seizoen. ↑/↓ kiest een aflevering, en Enter speelt die af, vanaf de opgeslagen plek als die er is.
- De focuslogica is dezelfde als die van de zenderlijst (groepen ↔ zenders).

### 3.5 Filmpagina (opzet F2)

**Links, net als bij de serie:**
- **Titel en kenmerken:** de titel, en ★ met bron, jaar, leeftijd, duur, en 4K/HDR en 5.1 als de portal ze meegeeft.
- **De knoppen:** ▶ Verder kijken (of Afspelen), Trailer, ＋, 👍 en 👎. Daaronder de voortgang ("nog 1 u 34 min · ↺ Vanaf het begin").
- **Versie:** "Versie: 4K HDR ▾" (§4.2), met een korte beschrijving en regie en cast.

**Rechts:** een raster "Meer zoals dit · <genre>". Daarin 6 titels uit hetzelfde genre of dezelfde categorie, best gewaardeerd en nog niet gezien, zonder 👎-titels.

**Toetsen:** → gaat naar het raster, Enter opent of speelt, en `v` opent de versiekeuze.

### 3.6 Tijdens het kijken (film of aflevering)

**Geldt alleen voor wat vanuit de thuisbioscoop speelt.** Een film die je met `f` start, speelt zoals nu, omdat `f` ongewijzigd blijft.

**De balk** verschijnt bij elke toets of muisbeweging en verdwijnt na 3 s. Hij toont:
- **boven:** ← en de titel; bij een aflevering "Serie · S2 A5 · Titel";
- **onder:** de tijdbalk met positie en buffer, de tijd erboven bij het verschuiven, "1:13:04 · nog 1:33:12", en de knoppen ⏪ ⏯ ⏩ · Audio en ondertitels · Volgende aflevering (alleen bij een aflevering) · volledig scherm.

**Toetsen:**

| Toets | Werking |
|---|---|
| Spatie | Pauze en verder. |
| ←/→ | 10 s terug of vooruit. Vasthouden herhaalt. Vandaag springen ←/→ 30 s; voor live terugkijken blijft dat zo. |
| ↑/↓ | 60 s. **Tijdens een film of aflevering zapt ↑/↓ nooit,** en ook geen andere toets. |
| `a` / `s` | Openen het menu met de echte audio- en ondertitelsporen uit mpv (`track-list`: `lang` en `title` per spoor). Daar kies je met ↑/↓ en Enter. Vandaag wisselen `a` en `s` alleen door. |
| `n` | Start de volgende aflevering. |
| Esc | Stopt, bewaart de plek en gaat terug naar de pagina. |

**Kijkgeheugen:**
- **Bewaren:** mpv meldt `time-pos` en `duration`. De plek gaat elke 5 s naar het kijkgeheugen (alleen bij een verandering) en bij stoppen.
- **Niet begonnen:** een plek onder 60 s telt als niet begonnen.
- **Gezien:** vanaf 90 % van de duur, of als het einde van het bestand bereikt is.

**Volgende aflevering:**
- **De kaart:** als er nog 30 s of minder over is, verschijnt rechtsonder "VOLGENDE AFLEVERING · S2 · A6 · Titel", met still, duur en een aftelling van 10 s (opzet uit `series-page.html`).
  - Enter speelt hem meteen.
  - Esc verbergt de kaart, zodat je de aftiteling uitkijkt.
  - Na de aftelling, of aan het eind van het bestand, start de volgende aflevering vanaf 0.
- **Na het laatste:**
  - Na de laatste aflevering van een seizoen volgt aflevering 1 van het volgende.
  - Na de laatste aflevering van de serie ga je terug naar de seriepagina, met "↺ Opnieuw kijken".

**Wegvallen:**
- **Eén keer opnieuw:** valt het bestand onverwacht weg vóór 95 % (end of file met reden eof), dan laadt de app het één keer opnieuw met `start=<laatste plek>`, en de gebruiker ziet niets.
- **Daarna een melding:** binnen dezelfde minuut nog eens geeft een melding, en je gaat terug naar de pagina.
- De stall-watchdog blijft voor VOD uit, zoals nu.

### 3.7 Trailer

- **Opzoeken:**
  - De YouTube-code komt uit de details van de titel (`youtube_trailer`).
  - De app controleert hem eerst: precies 11 tekens uit `A–Z a–z 0–9 _ -`. Een andere code geeft geen trailer.
  - Daarna roept de app `yt-dlp.exe` aan (naast de exe), zonder shell, met `--no-warnings -g -f "bv*[height<=1080]+ba/b[height<=1080]" -- https://www.youtube.com/watch?v=<code>`.
  - Ondertussen staat er "Trailer laden…".
- **Afspelen:**
  - Er komen één of twee regels terug: beeld, en eventueel apart geluid.
  - mpv speelt ze als `loadfile <beeld> replace -1 audio-file=<geluid>`.
  - Dat vervangt wat speelt; de tv stopt, net als bij een film.
- **Terug:** Esc of het einde brengt je terug naar de pagina. Er komt geen voortgang in het kijkgeheugen.
- **Mislukt:** lukt het niet binnen 15 s, of geeft yt-dlp een fout, dan stopt de app het proces en meldt "Trailer niet beschikbaar". Niets blokkeert het scherm.
- **Nooit vanzelf:** een trailer speelt alleen na een druk op de knop.

### 3.8 Mijn lijst, 👍 en 👎

| Knop | Betekenis |
|---|---|
| **＋ Mijn lijst** | Voor later kijken: een rij op Thuis en een tabblad. Nogmaals drukken haalt de titel eraf. |
| **👍 Leuk** | Voedt "Omdat je X leuk vond" (§3.2). Nogmaals drukken trekt het in. |
| **👎 Niet voor mij** | De titel verdwijnt uit de rijen, uit "Meer zoals dit" en uit Uitgelicht. Zoeken vindt hem nog wel, en nogmaals drukken trekt het in. 👍 en 👎 sluiten elkaar uit. |

Alles staat alleen op de pc van de gebruiker, per bron (§4.5).

## 4. Gegevens

### 4.1 Wat de portal levert

**Bij het laden van de catalogus.** Dat gebeurt nu al op de achtergrond, bij Pandora ±6 s, en de lijsten komen al binnen. De app bewaart meer velden en haalt twee kleine lijsten erbij:
- **Films (`get_vod_streams`):**
  - nu al bewaard: `stream_id`, `name`, `container_extension` en `is_adult`;
  - erbij: `stream_icon` (de cover), `rating`, `added` en `category_id`.
- **Series (`get_series`):**
  - nu al bewaard: `series_id`, `name` en `is_adult`;
  - erbij: `cover`, `rating`, `releaseDate` (of `release_date`), `genre`, `category_id` en `last_modified`.
  - De beschrijving en de cast uit deze lijst bewaart de app **niet** in de index (geheugen). Ze komen uit de details.
- **Nieuw:** `get_vod_categories` en `get_series_categories` (id → naam).

**Op aanvraag (§4.3):**
- **Film:** `get_vod_info&vod_id=…`, met de beschrijving, cast, regie, genre, duur, uitgavedatum, `youtube_trailer`, `backdrop_path` en `tmdb_id`, plus beeld en geluid (resolutie, kanalen) als de portal die geeft.
- **Serie:** `get_series_info&series_id=…`, met de info als hierboven, de seizoenen (nummer, naam, cover) en de afleveringen. Per aflevering: id, nummer, titel, `container_extension`, `info.duration`, `info.plot`, `info.movie_image` en `added`.

**Waarderingen** komen als "7.3", als 7.3 of als "". De app leest ze tolerant: een lege waarde of 0 betekent geen waardering.

### 4.2 De titel-index

**Een klein record per titel,** zonder tekst behalve een verwijzing naar de naam:

| Veld | Waarde |
|---|---|
| Soort | Serie of film |
| Id | `series_id`, of de `stream_id` van de standaardversie |
| Naam | Index in een gedeelde naamtabel, plus een genormaliseerde zoeknaam (kleine letters, zonder accenten) |
| Jaar | `u16`, 0 als het onbekend is |
| Waardering | `u8`, in tienden van 0 tot 100; 0 als er geen is |
| Genres | Een bitmasker over de vaste genrelijst |
| Categorie | Id |
| Toegevoegd | Unix-dag |
| Taal | NL, EN of anders, als het bekend is |
| Vlaggen | Volwassen; heeft een trailer (na de details) |
| Versies | Bij films: de lijst `stream_id`'s met hun kwaliteitslabel |

**De richtlijn:** hooguit 5 MB voor alle titels samen.

**Het jaar van een film:**
- Uit de naam: "(2024)" of " 2024" aan het eind. Daarna uit de details zodra die er zijn.
- **Bij series:** uit `releaseDate`.

**Genres:** een vaste lijst van 24, met NL/EN/DE-namen in `i18n.rs` en per genre aliassen ("Comedy", "Komedie" en "Komödie" worden één genre).
- **Bij series:** uit het veld `genre`.
- **Bij films:** uit de naam van de categorie als die een alias bevat ("NL | Komedie" wordt Komedie), en anders uit de details.

**Taal:** Nederlands als de categorie of de naam een NL-kenmerk heeft: "NL", "Nederlands", "Dutch", "NL Gesproken" of "[NL]", als los woord. Engels voor "EN", "English" of "UK". Anders onbekend.

**Versies samenvoegen (films):**
- **De sleutel** is de naam zonder jaar en zonder kwaliteits- of taallabels, genormaliseerd, plus het jaar.
- **De labels:** 4K, UHD, 2160p, HDR, 1080p, FHD, 720p, HD, SD, NL, NL SUBS, MULTI, DUAL, 3D, en varianten tussen haakjes of blokhaken.
- **Standaardversie:** de hoogste kwaliteit wordt de standaard, als `quality_rank` dat bij zenders doet. De keuze staat op de filmpagina.
- **Series** worden in deze ronde niet samengevoegd.

**Volwassen inhoud:** de index ligt achter de catalogus, die het slot bewaakt. Zolang het slot erop zit, staan volwassen titels niet in de index, de rijen, de zoekresultaten of Verder kijken. Er komt nergens anders een filter.

**Bouwen:**
- De laaddraad bouwt de index uit de lijsten, nooit de schermdraad.
- Hooguit 150 ms extra voor ±58.000 titels in een release-build.

### 4.3 Details

- **Ophalen:** op de achtergrond (tokio), zodra een pagina opent. Hooguit 2 tegelijk, de nieuwste vraag eerst.
- **Bewaren:** het antwoord gaat naar `config\cache\vod\<host>_<port>\<s|m><id>.json`. Daarin staan alleen de velden die de pagina toont, zonder adressen met inloggegevens.
- **Vernieuwen:** een serie wordt opnieuw gevraagd als het bestand ouder is dan 24 uur, een film na 30 dagen. Een oud bestand toont de app meteen, terwijl de verse vraag loopt.
- **De pagina** toont eerst wat de index weet: naam, ★ en jaar. Wat binnenkomt schuift in beeld, zonder dat iets verspringt.
- **Mislukt het ophalen,** dan staat er "Details konden niet geladen worden. Enter probeert opnieuw".

### 4.4 Covers, achtergronden en stills

**Bronnen:**
- `stream_icon` (film) en `cover` (serie) uit de lijsten;
- `backdrop_path` en `movie_image` (aflevering) uit de details.

**Ophalen:**
- Alleen wat in beeld is, plus de volgende rij of het volgende scherm van het raster.
- Hooguit 6 tegelijk. Wat uit beeld schuift voordat het binnen is, wordt afgebroken.
- Gebruik een eigen HTTP-client, gemaakt bij de eerste cover, zoals `guide/logos.rs` dat doet.

**Decoderen:**
- PNG en JPEG, door de `jpeg`-functie van de bestaande crate `image` aan te zetten. Er komt geen nieuwe crate bij.
- Het decoderen gebeurt op de blocking pool van tokio (`spawn_blocking`), nooit op de schermdraad en zonder een eigen thread.
- Verkleind naar een van drie vaste formaten, het eerste dat groter of even groot is als de getoonde tegel. Zo blijft de cache bruikbaar bij een ander venster.

| Soort | Formaten |
|---|---|
| Staande cover | 200×300, 300×450, 400×600 |
| Liggende tegel of still | 320×180, 480×270, 640×360 |
| Achtergrond | 1280×720, 1920×1080 |

**Textuurgeheugen:** hooguit 96 MB samen (breedte × hoogte × 4). Wat het langst niet getoond is, gaat er als eerste uit.

**Schijfcache:**
- De bytes zoals ze binnenkwamen, in `config\cache\covers\<eerste 16 hex van sha256(adres)>.<ext>`.
- Hooguit 500 MB. Bij het starten ruimt de app op de achtergrond het oudste op als er meer staat.
- Een cover-adres met de inloggegevens van de portal erin wordt nooit gelogd. De bestandsnaam is een hash, dus het adres staat ook niet op schijf.

**Geen cover:**
- Een tegel met de titel in het display-lettertype op een kleurvlak.
- De tint volgt uit de eerste bytes van `sha256(naam)`, dus een titel heeft altijd dezelfde kleur.
- De tegel heeft dezelfde maat als een cover, dus er verspringt niets als de cover binnenkomt.

### 4.5 Kijkgeheugen

**Het bestand** staat per bron, naast de favorieten: `watching.json` voor bron 1 en `watching-<id>.json` voor de andere, via `Storage::source_file`. Het schrijven gebeurt atomisch (tijdelijk bestand, dan hernoemen), hooguit één keer per 5 s, en altijd bij stoppen.

```json
{
  "version": 1,
  "items": {
    "m:12345": { "pos": 4380, "dur": 9960, "at": 1792089306, "done": false },
    "e:987654": { "pos": 1415, "dur": 2820, "at": 1792089400, "done": false, "series": 4321, "s": 2, "n": 5 }
  },
  "list":   [ { "k": "s", "id": 4321, "at": 1792089000 } ],
  "thumbs": { "s:4321": 1, "m:555": -1 }
}
```

- **Inhoud:** alleen nummers en tijden. Geen namen, geen adressen.
- **Films met versies:** de sleutel `m:<stream_id>` is die van de versie die speelde. Een filmtitel toont de recentste voortgang van al zijn versies. Komt er later een nieuwe standaardversie bij, dan blijft de voortgang dus staan.
- **Volwassen inhoud:** blijft weg via de index: wat de index niet toont, toont geen rij.
- **Een onleesbaar bestand** wordt bewaard als `watching.json.bad`. De app begint leeg en logt het één keer.
- **Een bron verwijderen** (`remove_source_files`) haalt ook dit bestand weg.

### 4.6 De index-cache

- **Na elke geslaagde lading** schrijft de app de index als `config\cache\vod-<host>_<port>.json`. Daarin staan geen volwassen titels (dezelfde regel als de EPG-cache) en geen inloggegevens.
- **Bij het starten** leest de app hem pas **na het eerste frame**, op de achtergrond. Zo is Thuis gevuld voordat de verse lijst binnen is.
- **Een onleesbare cache** wordt genegeerd en bij de volgende lading vervangen.
- **Thuis met oude gegevens:** komt de verse lijst niet binnen, dan toont Thuis die uit de cache, met een klein regeltje "Gegevens van <dag> <tijd>".

## 5. Snelheid en geheugen

Gemeten in een release-build op de pc van de gebruiker, met een catalogus van ±58.000 titels.

| Wat | Norm |
|---|---|
| App starten | Niet trager dan nu. A/B met `rust/e2e/measure-startup.ps1 -Baseline`: minstens 10 runs, mediaan en minimum binnen ±10 %, met een Xtream-bron. |
| Zappen | Ongewijzigd. Een gesloten thuisbioscoop kost per frame niets: geen werk, geen geheugen, geen netwerk. |
| Index bouwen | Hooguit 150 ms extra op de laaddraad en hooguit 5 MB geheugen. |
| `h` → Thuis getekend | Binnen 100 ms na de toets. Covers uit de schijfcache volgen binnen 300 ms. |
| Typen of een filter aan/uit → resultaten met aantallen | Binnen 16 ms (één frame), en nooit trager dan `f` nu op dezelfde machine. |
| Bewegen door rijen en raster | 60 fps, geen frame boven 16 ms. Decoderen gebeurt nooit op de schermdraad. |
| Pagina openen | Binnen 100 ms getekend met de gegevens van de index. Details uit de schijfcache in hetzelfde frame. |
| Geheugen | Cover-texturen hooguit 96 MB. Schijfcache voor covers hooguit 500 MB. |
| Netwerk | Details alleen bij het openen van een pagina, hooguit 2 tegelijk. Covers hooguit 6 tegelijk, alleen voor wat in beeld komt. Bij het starten niets extra, behalve de twee categorielijsten in de bestaande catalogusronde. |

**Hoe het gemeten wordt:**
1. **Het ijkpunt `f`.** Fase 1 laat `f` per toets een logregel schrijven: `search: <n> results in <ms> ms`. Dat is de enige verandering aan `f`, en de regel zegt niets over wat er gezocht is. De zoekpagina van `h` schrijft dezelfde regel, zodat ze naast elkaar in het log staan.
2. **Een tijdtest in release.** Een genegeerde test (`--ignored`, release) bouwt een kunstmatige catalogus van 58.000 titels. Hij meet het bouwen van de index, zoeken plus filteren plus aantallen per toets, en het zoeken van `f` op dezelfde gegevens, en drukt de getallen naast elkaar af. Hij draait bij elke fase.
3. **`h` meet zichzelf:** van toets tot het eerste frame van Thuis, en van Enter tot het eerste frame van een pagina, als debug-logregels.
4. **De start-A/B** draait aan het eind van elke fase.

**Haalt een fase een norm niet,** dan gaat ze niet ter test naar de gebruiker. De getallen gaan eerst naar de gebruiker, met wat eraan te doen is.

## 6. Foutafhandeling

| Situatie | Gedrag |
|---|---|
| De portal is traag of onbereikbaar | Thuis uit de index-cache, met "Gegevens van <dag> <tijd>". Covers uit de schijfcache. Details: "konden niet geladen worden, Enter probeert opnieuw". Nooit een leeg of hangend scherm. |
| Geen cover | Een tegel met titel en vaste kleur (§4.4). |
| Geen waardering (0 of leeg) | Geen sterren, nooit "★ 0". |
| Geen beschrijving | Die regel valt weg. |
| Geen titel van een aflevering | "Aflevering <n>". |
| Seizoen 0 | "Extra's". |
| De portal weigert een film of aflevering | Je blijft op de pagina met "Kan dit niet afspelen". Heeft de film een andere versie: "Probeer de versie <label> (Enter)". |
| De stream valt weg vóór 95 % | Eén keer opnieuw met `start=<plek>`; de tweede keer binnen een minuut een melding en terug naar de pagina (§3.6). |
| De trailer lukt niet (yt-dlp ontbreekt, is verouderd, of is na 15 s niet klaar) | "Trailer niet beschikbaar". Het proces wordt gestopt en niets blokkeert. |
| De index-cache is onleesbaar | Genegeerd; bij de volgende lading vervangen. |
| Het kijkgeheugen is onleesbaar | Bewaard als `.bad`, leeg verder, één logregel. |
| De schijf is vol of schrijven mislukt | Gelogd. Niets wordt weggegooid wat er stond, want schrijven gaat atomisch. |
| Wisselen van bron | Sluit de thuisbioscoop. Elke bron heeft een eigen index, eigen details en een eigen kijkgeheugen. |
| Een M3U-bron | `h` meldt "De thuisbioscoop werkt met een Xtream-bron". |
| Het slot gaat erop of eraf | De rijen en de resultaten worden opnieuw berekend. Een pagina van een volwassen titel sluit als het slot erop gaat. |

**Geheimen:**
- Een stream-adres bouwt de app pas bij het afspelen, met de bestaande `movie_stream_url` en `series_stream_url`.
- Het gaat naar mpv zoals nu, als `Cmd::LoadUrl` byte voor byte, met alleen per-bestand-opties (`start=`) erbij.
- Nooit een adres met het account in een log, cache of `Debug`-uitvoer. Nieuwe types met zo'n adres krijgen `cfg_attr(test, derive(Debug))`.

## 7. Architectuur

**Nieuw, de gegevens (`rust/src/vod/`, tekent niets):**

| Bestand | Verantwoordelijkheid |
|---|---|
| `vod/mod.rs` | Gedeelde types: `TitleId` (serie of film) en `Kind`. |
| `vod/index.rs` | `TitleIndex`: de records, de naamtabel, en zoeken, filters, aantallen en sorteren. Puur en zonder I/O. |
| `vod/genres.rs` | De vaste genrelijst, aliassen, en taal- en kwaliteitslabels. |
| `vod/versions.rs` | Films samenvoegen tot titels met versies. |
| `vod/details.rs` | `DetailStore`: ophalen op aanvraag, de schijfcache en de verversingsregels. |
| `vod/covers.rs` | `CoverStore`: ophalen, de schijfcache, decoderen op de blocking pool, en de textuur-LRU met plafond. |
| `vod/watching.rs` | `Watching`: het kijkgeheugen, Mijn lijst en duimen, met atomisch schrijven. |
| `vod/rows.rs` | De rijen van Thuis en de keuze van Uitgelicht. Puur, uit de index en het kijkgeheugen. |
| `vod/trailer.rs` | yt-dlp aanroepen (codecontrole, tijdslimiet, uitkomst lezen). Het proces gaat via `tokio::process` (de functie `process` van tokio, geen nieuwe crate). |
| `vod/cache.rs` | De index-cache (§4.6). |

**Nieuw, de schermen (`rust/src/home_ui/`):**

| Bestand | Verantwoordelijkheid |
|---|---|
| `home_ui/mod.rs` | `HomeView` en `HomeAction`, zoals `GuideView` en `GuideAction`. |
| `home_ui/thuis.rs` | Thuis (opzet A). Series en Films gebruiken dezelfde rijen. |
| `home_ui/search.rs` | De zoekpagina (Z2). |
| `home_ui/series_page.rs` | De seriepagina (opzet 2). |
| `home_ui/film_page.rs` | De filmpagina (F2). |
| `home_ui/osd.rs` | De balk tijdens het kijken, het menu voor audio en ondertitels, en de kaart "volgende aflevering". |
| `home_ui/widgets.rs` | De cover-tegel, de focus met vergroting en animatie, knoppen, labels en de tijdbalk. |
| `home_ui/theme.rs` | Kleuren, maten en lettertypes. |

**Lettertypes:** Bebas Neue (titels) en Inter Regular en Bold (tekst), beide SIL Open Font License.
- Ingebakken met `include_bytes!` en verkleind tot Latijn, Latin-1 en de gebruikte tekens (★ ✓ ▶ ⏸ ↺ ＋ ✕ ▾). Samen hooguit 400 KB.
- Alleen de thuisbioscoop gebruikt ze; de rest van de app houdt zijn lettertype.
- Iconen zijn vectorvormen van egui of de ingebouwde emoji-iconen van egui, geen kleuren-emoji.

**Aanpassingen aan bestaande code:**
- **`portal/types.rs` en `portal/xtream.rs`:** de extra velden en de twee categorielijsten (§4.1), en `get_vod_info` met de volledige serie-info (die nu alleen afleveringen leest).
- **`catalog.rs`:** de `TitleIndex` naast live, films en series. Het slot geldt ook voor de index. Hij wordt gebouwd op de laaddraad.
- **`player/`:** per-bestand-opties `start=` en `audio-file=` bij een VOD-load (het adres blijft byte voor byte), `track-list` lezen, `aid` en `sid` kiezen, `time-pos` en `duration` volgen, en `mute`.
- **`app.rs`:**
  - `h` en de thuisbioscoop;
  - de tv stil en onzichtbaar;
  - geen zap tijdens VOD;
  - het kijkgeheugen bijwerken;
  - volgende aflevering;
  - één keer herladen bij eof;
  - terug naar tv met een zap.
- **`app.rs` en `search.rs`:** de logregel per toets in `f`, het ijkpunt (§5). Verder verandert er niets aan `f`.
- **`storage.rs`:** `watching_path` en `remove_source_files`.
- **`Cargo.toml`:** `image` krijgt `jpeg` en `tokio` krijgt `process`. Geen nieuwe crate.
- **`build-zip.ps1`:** yt-dlp.exe meeleveren, een vastgezette versie met controlesom (licentie: The Unlicense).

**Draden:** geen nieuwe thread. De index komt op de bestaande laaddraad. Details, covers en de trailer lopen op de bestaande tokio-runtime, en decoderen op de blocking pool.

## 8. Testen

- **Unit-tests, puur en zonder netwerk:**
  - de index (zoeken, filters, aantallen, sorteren, zonder accenten);
  - genres, aliassen en labels, en het samenvoegen van versies;
  - de rijen en Uitgelicht;
  - het kijkgeheugen (lezen, schrijven, `.bad`, de 90 %-regel, de 60 s-regel);
  - de verversingsregels van de details;
  - de cover-LRU met nep-texturen;
  - de codecontrole van de trailer.
- **App-tests met de headless `Harness`:**
  - `h` opent en sluit;
  - de tv krijgt `mute` en wordt niet getekend, en komt terug;
  - tabbladen, rijen, seizoenen en afleveringen doorlopen;
  - wat er naar mpv gaat: het adres byte voor byte, plus `start=`;
  - de aftelling van de volgende aflevering, en het oversteken naar een volgend seizoen;
  - één keer herladen bij eof;
  - ↑/↓ zapt nooit tijdens VOD;
  - het slot sluit volwassen pagina's;
  - Esc-stappen en terug naar tv met een zap.
- **Gegevens van de portal** komen uit nagemaakte JSON in de vorm van Xtream, nooit uit de echte portal.
- **Venstertests tegen `rust/e2e/mock_portal.py`.**
  - De mock krijgt films (met dubbele versies), series met seizoenen en afleveringen, de categorieën, `get_vod_info` en `get_series_info`, en lokale covers (PNG en JPEG).
  - Er komt een nep-yt-dlp: een script dat een lokaal videoadres teruggeeft, of bewust te laat of fout antwoordt.
  - Alleen gepost toetsenbord, alleen tegen de mock, en alleen met toestemming van de gebruiker, zoals nu.
- **De tijdtests van §5,** in release, met `f` als ijkpunt.

## 9. Fasen

Elke fase heeft een eigen plan en wordt per taak gebouwd en gecontroleerd, zoals de zenderlijst. Ze eindigt met:
- een release-build die de gebruiker onder `h` test;
- de metingen van §5;
- een start-A/B.

**Fase 1, het fundament.** Nog niets zichtbaar, behalve de logregel van `f`.
- **Inhoud:**
  - de extra lijstvelden en categorieën;
  - de titel-index met genres, labels, versies en cache;
  - de opslag voor details en covers (met JPEG);
  - het kijkgeheugen;
  - de uitbreidingen aan `player/`;
  - de tijdtest met het ijkpunt van `f`.
- **Klaar als:** alle tests groen zijn, de index de normen voor bouwen, geheugen en zoeken haalt (≤ `f`), en het starten niet trager is.

**Fase 2, kijken.** De eerste fase die de gebruiker kan testen.
- **Inhoud:**
  - `h` met de zoekpagina (Z2), de seriepagina (2) en de filmpagina (F2). Er is dan nog alleen het tabblad Zoeken, dus `h` opent daar direct. De pagina's hebben nog geen Trailer-knop (fase 3);
  - afspelen met verder kijken;
  - de balk en de toetsen tijdens het kijken, het menu voor audio en ondertitels;
  - de volgende aflevering, en één keer herladen.
- **Klaar als:** de gebruiker op serienaam kan zoeken, kan filteren, een seizoen en aflevering kan kiezen, kan kijken en verder kijken, en de normen gehaald zijn.

**Fase 3, Thuis.**
- **Inhoud:**
  - Thuis (A) met Uitgelicht en alle rijen;
  - de tabbladen Series, Films en Mijn lijst;
  - ＋, 👍 en 👎, met "Omdat je … leuk vond";
  - de trailer via yt-dlp.
- **Klaar als:** alle schermen van de schetsen werken met de normen, en de trailer speelt in de venstertest met de nep-yt-dlp.

## 10. Buiten deze ronde

- **Ontwerp C:** OMDb en TMDB als aanvulling bij ontbrekende gegevens, met bronvermelding. Beide vragen een (gratis) sleutel. TMDB is gratis voor niet-commercieel gebruik met logo en de zin "This product uses the TMDB API but is not endorsed or certified by TMDB" in een Over- of Credits-scherm; voor commercieel gebruik is een licentie nodig. Testen gebeurt tegen een mock.
- **Live-zenders** in de zoekpagina van `h`, en `f` omzetten naar die pagina. Dat is een latere beslissing van de gebruiker, na het testen.
- **Overig, nu niet:**
  - categorieën zelf vastzetten op Thuis;
  - versies van series samenvoegen;
  - zoeken op acteur;
  - profielen;
  - downloaden;
  - casten;
  - intro overslaan.
- **M3U-films in de thuisbioscoop,** na de rijpheidsronde, die M3U-films als films laat spelen.

## 11. Risico's en open punten

- **YouTube en yt-dlp.**
  - YouTube staat in zijn voorwaarden alleen afspelen via zijn eigen speler of API toe. Voor een product dat verkocht wordt is yt-dlp daarom een **commercieel en juridisch risico**: een zakelijke keuze van de gebruiker vóór de verkoop.
  - yt-dlp moet regelmatig bijgewerkt worden. Een verouderde versie geeft "Trailer niet beschikbaar", en een nieuwe app-versie levert een nieuwere yt-dlp mee.
- **De kwaliteit van Pandora's gegevens** is pas bij de eerste echte titels te zien. Het ontwerp valt overal netjes terug (§6), en fase 1 meldt het aandeel titels met een cover, een waardering, een jaar en een genre.
- **Covers** komen van de portal, vaak als TMDB-afbeeldingen. Bij een commerciële versie hoort bronvermelding of een licentie (samen met ontwerp C).
- **Het moois van de schetsen** (verlopen, schaduwen, vergroting, vloeiend scrollen) is in egui handwerk. Fase 2 en 3 plannen daar tijd voor. De normen van §5 gaan altijd voor.
- **Geheugen:** de plafonds van §4.4 zijn een eerste inschatting. Fase 2 meet het echte gebruik op de pc van de gebruiker.
