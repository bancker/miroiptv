# Bongo IPTV Player op macOS — ontwerp

Datum: 2026-10-16. Bouwt voort op `main` na de hernoeming (`2026-10-16-hernoemen-design.md`). Branch:
`feat/macos`, te beginnen na fase 1 van de thuisbioscoop.

## 1. Doel

De gebruiker vroeg: "maak als je klaar bent een spec voor macos". Dit ontwerp maakt van dezelfde app
een Mac-versie: dezelfde functies (Xtream en M3U, de gids, de zenderlijst, zoeken, favorieten,
sneltoetsen, het slot op volwassen inhoud, en de thuisbioscoop zodra die er is), als `.app` voor Intel
en Apple Silicon.

De Windows-versie blijft zoals ze is. Niets wordt daar trager, en de Windows-gebruiker merkt niets.

Geslaagd is het als:
1. de app op de Intel-Mac van de gebruiker start, Xtream en M3U afspeelt en zapt;
2. dezelfde `.app` op Apple Silicon start en afspeelt;
3. alle tests die zonder venster draaien op de Mac groen zijn;
4. de Windows-build, de Windows-tests en de Windows-metingen ongewijzigd groen blijven.

## 2. Wat de gebruiker koos

| Onderwerp | Keuze |
|---|---|
| Een Mac om te bouwen en te testen | Ja, een Intel-Mac |
| Welke Macs | Ook Intel: één app voor Intel en Apple Silicon |
| Ondertekenen en notariseren bij Apple | Later beslissen. Eerst ongetekend voor eigen gebruik; dit ontwerp beschrijft beide wegen |
| Het beeld | Eerst zoals nu: de bestaande softwareweg. Scherp beeld via de GPU volgt als eigen stap, met metingen |

## 3. De app op macOS

### 3.1 Wat hetzelfde blijft

- Alle schermen, toetsen en teksten, in NL, EN en DE.
- De bestanden en hun formaat: de ini, de favorieten, de sneltoetsen, de defecte zenders, de caches.
- De netwerkregels: de relay voor live Xtream, één sessie per account, het zaptempo.
- De veiligheidsregels: geen adres met inloggegevens in een log, een cache of `Debug`-uitvoer.
- Eén codebasis en één crate. Wat per platform verschilt, staat achter
  `cfg(target_os = "macos")`, zoals het Windows-deel nu achter `cfg(windows)` staat.

### 3.2 Waar de bestanden staan

- **De gegevensmap** is `~/Library/Application Support/BongoIPTVPlayer/`. Dat is wat de crate
  `directories` op de Mac geeft, zoals `%APPDATA%\BongoIPTVPlayer\config\` op Windows. De logs staan
  daaronder in `log/`, zoals op Windows.
- **De ini** staat op Windows naast de exe: de app is daar portable. In een `.app` kan dat niet,
  want de bundel is alleen-lezen en elke wijziging erin breekt de handtekening. Op de Mac staat
  `BongoIPTVPlayer.ini` daarom in de gegevensmap.
  - `BONGO_INI` en `BONGO_CONFIG_DIR` werken als op Windows.
  - Het startscherm zegt nu dat de gegevens "naast de app" komen. Op de Mac zegt het "in de
    gegevensmap van de app", met een tekst van zijn eigen in NL, EN en DE.
- **Meenemen van vroegere namen** gebeurt op de Mac niet: er was nog geen Mac-versie.

### 3.3 Toetsen en venster

- Alle lettertoetsen werken als op Windows.
- **Volledig scherm:** F11 blijft, en Ctrl+Cmd+F komt erbij, de toets die Mac-apps gebruiken. Op
  veel Macs is F11 "Toon bureaublad" of vraagt hij de Fn-toets.
- **Cmd+Q** sluit de app, via het app-menu dat macOS zelf geeft.
- **Randloos en altijd bovenop** (`t`) gaan via egui, zoals op Windows.
- **Slepen met de rechtermuisknop:** op Windows leest `window_drag.rs` de cursor en de knoppen via
  user32. Op de Mac doet een klein blok hetzelfde via CoreGraphics: `CGEventGetLocation` voor de
  cursor en `CGEventSourceButtonState` voor de knop. Er komt geen nieuwe crate bij. Op een trackpad
  is rechts slepen: klikken met twee vingers en dan slepen. Of dat handig genoeg is, beslist de
  gebruiker na het testen.
- **`--console`** is op de Mac niet nodig: wie de app uit Terminal start, ziet de logs al. De vlag
  doet daar niets.

### 3.4 De taal bij de eerste start

- Op Windows komt de voorkeuze van de taal uit `GetUserDefaultUILanguage`. Op andere systemen kiest de
  app nu altijd Engels.
- Op de Mac komt ze uit de eerste voorkeurstaal van het systeem (`CFLocaleCopyPreferredLanguages`):
  "nl-…" wordt NL, "de-…" DE, de rest EN. Ook dit is een klein blok zonder nieuwe crate. `LANG`
  helpt niet: een app die uit Finder start, krijgt hem meestal niet.

### 3.5 Beeld en geluid

- **Dezelfde weg als op Windows** (de keuze van de gebruiker): libmpv tekent elk frame in software in
  een RGBA-plaatje, en egui toont dat als textuur.
- **Op een Retina-scherm** tekent mpv op de venstermaat in punten, dus op de helft van de pixels van
  het scherm. Het beeld is daardoor iets zachter. De processor doet dan evenveel werk als op Windows
  bij een schaal van 100 %. Scherp beeld via de GPU is een eigen stap, later (§9).
- **Hardwaredecodering:** op Windows staat `hwdec=auto-safe`. Met de softwareweg werkt VideoToolbox op
  de Mac alleen in de kopieervorm (`videotoolbox-copy`). Fase 1 meet wat mpv met `auto-safe` kiest, en
  zet `videotoolbox-copy` als dat sneller of zuiniger is. De norm is de tijd van zappen tot beeld, en
  het processorgebruik tijdens het kijken.
- **Geluid** gaat via mpv's eigen uitgang voor macOS. De app doet daar niets voor.

## 4. libmpv voor de Mac

- **Wat:** `libmpv.2.dylib` en de bibliotheken die hij nodig heeft (ffmpeg, libass en zo verder), voor
  Intel (x86_64) en voor Apple Silicon (arm64). Met `lipo` worden het universele bestanden.
- **Waar vandaan:** twee wegen. Fase 1 probeert de snelle, fase 2 kiest.
  - **Homebrew:** mpv van Homebrew op de Intel-Mac, en de arm64-versie ernaast opgehaald
    (`brew fetch --bottle-tag=…`). `dylibbundler` zet ze in de app. Dat is de snelste weg voor eigen
    gebruik, maar Homebrew bouwt met GPL-onderdelen, en beide architecturen moeten precies dezelfde
    versies hebben.
  - **Zelf bouwen:** mpv en ffmpeg uit de broncode, voor beide architecturen, met een script in
    `rust/vendor/`. Dat geeft volledige controle, ook over de licentie (§11). Het kost het meeste
    werk.
- **Meegeleverd zoals op Windows:** de binaire bestanden staan git-ignored in
  `rust/vendor/libmpv-macos/`, en een script om ze op te halen of te bouwen staat in git.
- **De licentie** volgt de keuze voor Windows. Daar staat een LGPL-build van libmpv al als eigen
  ronde gepland. Voor eigen gebruik is een GPL-build geen probleem.

## 5. Bouwen

- **`rust-toolchain.toml`** pint nu `stable-x86_64-pc-windows-gnu`. Die toolchain kan op een Mac niet
  draaien. Hij wordt `channel = "stable"`.
  - Windows houdt de GNU-toolchain als standaard van rustup (`rustup set default-host
    x86_64-pc-windows-gnu`, eenmalig per pc) en zijn `rust/.cargo/config.toml`.
  - Dat raakt beide Windows-pc's. Ze worden eerst omgezet en getest, en pas dan verandert de pin.
- **`build.rs`** linkt per platform. Windows blijft zoals het is. De Mac zoekt libmpv in
  `rust/vendor/libmpv-macos/` en zet de rpath `@executable_path/../Frameworks`.
- **`rust/build-app.sh`** (nieuw, bash, op de Mac) maakt de app:
  1. `cargo build --release` voor `x86_64-apple-darwin` en voor `aarch64-apple-darwin`;
  2. `lipo -create` maakt er één universeel programma van;
  3. de bundel: het programma in `Contents/MacOS/`, libmpv en zijn bibliotheken in
     `Contents/Frameworks/` (met `@rpath`-namen), het icoon in `Contents/Resources/`, en `Info.plist`
     met de bundel-id, de versie uit `Cargo.toml` en de laagste macOS-versie;
  4. ondertekenen (§6);
  5. de zip: `rust/dist/BongoIPTVPlayer-vX.Y.Z-macos.zip`, gemaakt met `ditto`, zodat de kenmerken
     van de bundel heel blijven.
- **De Windows-scripts** (`build-zip.ps1`, `verify.ps1`, de e2e) blijven zoals ze zijn.
- **Buiten Dropbox bouwen:** op Windows liep `cargo build` in de Dropbox-map vast op bestanden die
  Dropbox vasthield. Op de Mac komt de doelmap van cargo daarom ook buiten Dropbox.

## 6. Ondertekenen (de gebruiker beslist later)

- **Nu: zonder account.** Apple Silicon draait geen code zonder handtekening, dus de app krijgt een
  ad-hoc-handtekening (`codesign -s -`). Elke bibliotheek die `install_name_tool` aanpast, wordt
  opnieuw ondertekend.
  - De eerste keer zegt macOS dat de app niet geopend kan worden. Openen gaat dan via
    Systeeminstellingen, Privacy en beveiliging, "Open toch". Sinds macOS 15 werkt rechtsklikken en
    Openen daar niet meer voor.
  - Wie de app zelf heeft gedownload, kan ook het quarantainekenmerk weghalen:
    `xattr -dr com.apple.quarantine "Bongo IPTV Player.app"`.
- **Later, met een account** (99 dollar per jaar): een Developer ID-certificaat, de hardened runtime
  met het recht op netwerkverkeer, notarisatie via `notarytool`, en de uitkomst vastgeniet in de app
  (`stapler`).
  - `build-app.sh` krijgt daarvoor een schakelaar. Verder verandert er niets.
  - De bundel-id kiest de gebruiker dan.

## 7. Snelheid en normen

- **Windows:** de normen van altijd. Opstarten en zappen worden niet trager, gemeten met de A/B van
  `measure-startup.ps1`.
- **De Mac** krijgt een eigen nulmeting: de eerste versie die alles kan, op de Intel-Mac van de
  gebruiker, tegen de nep-portal.
  - Gemeten worden de tijd van starten tot beeld en van zappen tot beeld (10 keer, mediaan en
    minimum) en het processorgebruik tijdens een uur kijken.
  - Elke latere wijziging mag die nulmeting niet meer dan 10 % vertragen, zoals op Windows.
- **Geheugen en draden** volgen de regels van Windows: geen nieuwe thread, overlays die niets kosten
  als ze dicht zijn.

## 8. Testen

- **Op de Intel-Mac:**
  - `cargo test`: alle tests zonder venster. De app-tests draaien op een egui zonder scherm, dus
    vrijwel alles draait daar ook.
  - De libmpv-tests (ignored) met de testsegmenten van de e2e. `make-segments.ps1` krijgt een
    bash-versie, of de segmenten worden van Windows gekopieerd.
  - `cargo clippy`.
- **Apple Silicon:** de gebruiker heeft geen Apple Silicon-Mac.
  - Een macOS-server van GitHub (`macos-14`, Apple Silicon) draait `cargo test` voor arm64 en start
    de universele app met `--selftest`. Een venster is daar niet te zien.
  - Voor een release kijkt iemand met een Apple Silicon-Mac met de hand.
- **Windows:** alles zoals nu: de tests, de venstertests en de A/B.
- **De venstertests** bestaan alleen voor Windows: PowerShell en Win32-berichten. De Mac krijgt een
  handmatige lijst in `docs/VALIDATION.md`, sectie macOS. Een harnas voor de Mac is een eigen ronde
  (§9).
- **Tests van de Mac-blokken** (de taal, de cursor, het pad van de ini) staan achter
  `cfg(target_os = "macos")`, zoals de Windows-tests achter `cfg(windows)`.

## 9. Fasen

**Fase 1, het draait op de Mac.** Een ontwikkelversie op de Intel-Mac:
- de toolchain-pin, eerst op beide Windows-pc's;
- `build.rs` en libmpv via Homebrew;
- de ini in de gegevensmap, de taal, het slepen, Ctrl+Cmd+F en de tekst van het startscherm;
- de meting van `hwdec`;
- alle tests groen op de Mac, en een controle met de echte portal van de gebruiker.

**Fase 2, een echte app.**
- Universeel en ad-hoc ondertekend, met libmpv voor beide architecturen in de bundel.
- `build-app.sh` en de zip.
- De macOS-server van GitHub voor arm64.
- De handmatige lijst op de Intel-Mac, en de nulmeting.

**Later, elk apart:**
- ondertekenen en notariseren, als de gebruiker een account neemt;
- scherp beeld op Retina via de GPU (mpv's OpenGL-renderweg of een andere), met metingen tegen de
  softwareweg;
- een harnas voor venstertests op de Mac;
- de LGPL-build van libmpv, samen met Windows.

## 10. Niet in deze ronde

- De C-app (`miroiptv`) op de Mac.
- Een versie voor de Mac App Store: die vraagt een sandbox, en de app schrijft dan nergens buiten zijn
  eigen container.
- Linux.
- Ondertekenen en notariseren, tot de gebruiker beslist.
- De GPU-renderweg.

## 11. Risico's en open punten

- **libmpv voor twee architecturen** is het grootste stuk werk. Homebrew's versies voor arm64 en
  x86_64 moeten precies gelijk zijn, anders past `lipo` ze niet samen.
- **De licentie:** Homebrew's mpv en ffmpeg zijn GPL. Wie de app aan anderen geeft, moet dan ook de
  broncode aanbieden. Voor eigen gebruik geeft dat niets; de LGPL-build lost het op.
- **Apple Silicon is alleen elders te testen**, en op de server van GitHub zonder venster. Een
  probleem dat je alleen ziet, zoals in het beeld of in de toetsen, komt pas op een geleende Mac
  boven.
- **De laagste macOS-versie** moet fase 1 nog vaststellen. Het voorstel is macOS 11, de eerste versie
  die op Apple Silicon draait. egui heeft Metal nodig, en de libmpv van nu een recente macOS. macOS 26
  is de laatste versie die nog op Intel draait.
- **`hwdec` met de softwareweg** moet fase 1 meten (§3.5).
- **Een icoon:** de Rust-app heeft er nog geen, ook niet op Windows. De Mac-app heeft er een nodig
  (`.icns`). Het ontwerp ervan is een vraag aan de gebruiker.
- **F11 en de Fn-toets:** of Ctrl+Cmd+F genoeg is, beslist de gebruiker na fase 1.
- **De toolchain-pin** raakt beide Windows-pc's. Elke pc zet eenmalig zijn standaard, en pas daarna
  verandert de pin.

## 12. Bestanden (indicatief)

| Bestand | Wat |
|---|---|
| `rust-toolchain.toml` | `channel = "stable"`, zonder vaste Windows-toolchain |
| `rust/build.rs` | linken per platform; de rpath op de Mac |
| `rust/src/args.rs` | het pad van de ini op de Mac: de gegevensmap |
| `rust/src/i18n.rs` | de taal op de Mac; de tekst van het startscherm per platform |
| `rust/src/window_drag.rs` | een blok CoreGraphics naast het blok user32 |
| `rust/src/app.rs` | Ctrl+Cmd+F |
| `rust/src/main.rs` | `--console` doet op de Mac niets |
| `rust/src/player/mpv.rs` | `hwdec`, als de meting dat vraagt |
| `rust/vendor/fetch-mpv-macos.sh` of `build-mpv-macos.sh` (nieuw) | libmpv voor beide architecturen |
| `rust/build-app.sh` (nieuw) | de `.app`, ondertekend, en de zip |
| `rust/macos/Info.plist` (nieuw) | het sjabloon van de bundel |
| `.github/workflows/macos.yml` (nieuw) | de tests en `--selftest` op Apple Silicon |
| docs | `CLAUDE.md`, `rust/README.md`, `docs/VALIDATION.md` (sectie macOS) |
