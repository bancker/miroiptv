# Hernoemen naar Bongo IPTV Player — ontwerp

Datum: 2026-10-16. Branch: `feat/hernoemen`, op `main` na 0.7.0, de audiofix en de nieuwe indeling
van de zenderlijst.

## 1. Doel

De app heet voortaan **Bongo IPTV Player**. Het plan voor fase 1 van de thuisbioscoop
(`2026-10-15-thuisbioscoop-fase1.md`) gaat al van die naam uit.

## 2. Wat de gebruiker koos

- **De volgorde:** eerst hernoemen, dan de thuisbioscoop ("Eerst hernoemen").
- **De schermnaam:** "Bongo IPTV Player", passend bij de app-id `BongoIPTVPlayer`, zoals "Bongo
  Player" bij `BongoPlayer` hoorde.

## 3. Wat er verandert

- `APP_NAME` wordt "Bongo IPTV Player": de titelbalk, de hulp, het lege scherm en `--version`.
- `APP_ID` wordt `BongoIPTVPlayer`. Dat noemt:
  - de exe: `BongoIPTVPlayer.exe`;
  - de ini naast de exe: `BongoIPTVPlayer.ini`;
  - de configmap: `%APPDATA%\BongoIPTVPlayer\config\`;
  - het logbestand: `BongoIPTVPlayer.log.<datum>`;
  - de User-Agent naar de portal en de lijsten: `BongoIPTVPlayer/<versie>`.
- De zip heet `BongoIPTVPlayer-vX.Y.Z.zip`, met `run.bat` en de README op de nieuwe naam.
- Het startscherm voor de eerste portal noemt `BongoIPTVPlayer.ini`, in NL, EN en DE.

## 4. Wat blijft

- De namen van de crate (`bongo-player`, `bongo_player`). De gebruiker ziet ze niet, en de tests en
  het standaard logfilter verwijzen ernaar.
- De omgevingsvariabelen `BONGO_CONFIG_DIR` en `BONGO_INI`.
- Het formaat van de ini en van alle bestanden in de configmap.
- De versie. Die kiest de gebruiker bij de release.

## 5. Meenemen bij de eerste start

Zoals bij de vorige hernoeming (tvplayer naar Bongo Player), nu met twee vroegere namen, de nieuwste
eerst (`legacy::LEGACY_APP_IDS`: `BongoPlayer`, dan `tvplayer`):

- **De configmap** van `BongoPlayer` wordt gekopieerd, en anders die van `tvplayer`.
  - De map `cache\` gaat mee: de bewaarde M3U-lijsten, de gidscache en de logo's. Zo is de eerste
    start na de hernoeming net zo snel als de start ervoor.
  - De map `log\` gaat niet mee.
  - De oude map blijft staan, als reserve.
- **De ini:** `BongoPlayer.ini` naast de exe, en anders `tvplayer.ini`, wordt verplaatst naar
  `BongoIPTVPlayer.ini`. Zo is er altijd één ini.
- **Een kopie die mislukt** wordt bij de volgende start opnieuw geprobeerd, van dezelfde naam. Een
  oudere naam neemt nooit de plaats in van een nieuwere waarvan de kopie mislukte: dan zou de
  gebruiker oude gegevens krijgen en zou de nieuwere nooit meer meekomen.
- **Onder de testmap** (`BONGO_CONFIG_DIR`) wordt niets meegenomen, zoals nu.

## 6. Wat de gebruiker merkt

- Een snelkoppeling naar `BongoPlayer.exe` start de oude versie, en die vindt na de eerste start van
  de nieuwe geen ini meer. Start voortaan `BongoIPTVPlayer.exe` of `run.bat`.
- Terug naar 0.6.1 of 0.7.0: kopieer `BongoIPTVPlayer.ini` als `BongoPlayer.ini` naast die exe.

## 7. Scripts en tests

- **e2e:** het harnas start `BongoIPTVPlayer.exe` en leest beide lognamen (`Bongo*Player.log.*`),
  want de opstart-A/B draait 0.6.1 door hetzelfde harnas. In zijn testmappen houdt het de
  bestandsnaam `BongoPlayer.ini`: `BONGO_INI` noemt hem, voor elke exe. N-J1 verwacht de logregel met
  `BongoIPTVPlayer.ini`.
- **`check-staged.sh`** leest `BongoIPTVPlayer.ini` naast de release-exe, en anders
  `BongoPlayer.ini`: die staat er tot de nieuwe app er één keer gestart is.
- **`build-zip.ps1`, `verify.ps1` en `bongo.bat`** volgen de nieuwe naam.
- **Tests:**
  - de migratie: de nieuwste naam eerst, de cache mee, de log niet, geen terugval op een oudere
    naam na een mislukte kopie, de ini één keer;
  - de User-Agent van de portal en van een lijst;
  - het pad van de ini, met en zonder testmap.
