//! Every text the user reads, in Dutch, English and German. Channel and
//! programme names come from the portal and are not translated; log lines
//! stay English.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Nl,
    En,
    De,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::Nl, Lang::En, Lang::De];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Nl => "nl",
            Lang::En => "en",
            Lang::De => "de",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        match code.trim().to_ascii_lowercase().as_str() {
            "nl" => Some(Lang::Nl),
            "en" => Some(Lang::En),
            "de" => Some(Lang::De),
            _ => None,
        }
    }

    /// The language's own name, for the language choice.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Nl => "Nederlands",
            Lang::En => "English",
            Lang::De => "Deutsch",
        }
    }
}

/// Windows display language → `Lang`: Dutch and German map to themselves,
/// everything else to English.
pub fn lang_from_langid(langid: u16) -> Lang {
    match langid & 0x3ff {
        0x13 => Lang::Nl,
        0x07 => Lang::De,
        _ => Lang::En,
    }
}

/// The Windows display language, for the first-start preselection.
#[cfg(windows)]
pub fn system_lang() -> Lang {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: no arguments, returns a LANGID by value.
    lang_from_langid(unsafe { GetUserDefaultUILanguage() })
}

#[cfg(not(windows))]
pub fn system_lang() -> Lang {
    Lang::En
}

macro_rules! texts {
    ($($key:ident => [$nl:expr, $en:expr, $de:expr],)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum T { $($key,)* }

        impl T {
            pub const ALL: &'static [T] = &[$(T::$key,)*];
        }

        fn table(key: T) -> [&'static str; 3] {
            match key { $(T::$key => [$nl, $en, $de],)* }
        }
    };
}

texts! {
    // Playback and zapping
    ResumingLast => ["laatste zender hervatten...", "resuming last channel...", "letzten Sender fortsetzen..."],
    Reconnecting => ["herverbinden...", "reconnecting...", "neu verbinden..."],
    ChannelNotInSubscription => ["zender niet beschikbaar voor dit abonnement", "channel not available on this subscription", "Sender in diesem Abo nicht verfügbar"],
    StreamErrorRetry => ["streamfout - zap omhoog of omlaag om opnieuw te proberen", "stream error - zap up or down to retry", "Streamfehler - zum erneuten Versuch hoch oder runter zappen"],
    StreamErrorOn => ["streamfout op {0} - zap omhoog of omlaag om opnieuw te proberen", "stream error on {0} - zap up or down to retry", "Streamfehler auf {0} - zum erneuten Versuch hoch oder runter zappen"],
    ErrorMsg => ["fout: {0}", "error: {0}", "Fehler: {0}"],
    PauseLiveOnly => ["pauze: alleen bij films, series en terugkijken", "pause: only for films, series and catch-up", "Pause: nur bei Filmen, Serien und Wiederholungen"],
    Paused => ["|| pauze  -  spatie om verder te kijken", "|| paused  -  space to continue", "|| Pause  -  Leertaste zum Fortsetzen"],
    Resumed => ["> verder", "> playing", "> weiter"],
    VariantUnavailable => ["[TV] {0} ({1} niet beschikbaar)", "[TV] {0} ({1} not available)", "[TV] {0} ({1} nicht verfügbar)"],
    CatalogLoading => ["catalogus laden...", "loading catalog...", "Katalog wird geladen..."],
    NoLiveChannels => ["geen live-zenders in de catalogus", "no live channels in the catalog", "keine Live-Sender im Katalog"],
    QualityNoLive => ["kwaliteit: geen live-zender", "quality: no live channel", "Qualität: kein Live-Sender"],
    QualityNoOther => ["kwaliteit: geen andere variant", "quality: no other variant", "Qualität: keine andere Variante"],
    QualityUnknown => ["kwaliteit: huidige variant onbekend", "quality: current variant unknown", "Qualität: aktuelle Variante unbekannt"],
    QualityLowest => ["kwaliteit: al op laagste", "quality: already lowest", "Qualität: bereits niedrigste"],
    QualityHighest => ["kwaliteit: al op hoogste", "quality: already highest", "Qualität: bereits höchste"],
    QualityUp => ["kwaliteit hoger: {0}", "quality up: {0}", "Qualität höher: {0}"],
    QualityDown => ["kwaliteit lager: {0}", "quality down: {0}", "Qualität niedriger: {0}"],
    TagMovie => ["FILM", "MOVIE", "FILM"],
    TagSeries => ["SERIE", "SERIES", "SERIE"],
    TagReplay => ["TERUG", "REPLAY", "REPLAY"],
    CycleAudio => ["audiospoor wisselen", "next audio track", "nächste Tonspur"],
    CycleSubtitle => ["ondertiteling wisselen", "next subtitles", "nächste Untertitel"],
    RetryingPortal => ["portal opnieuw ophalen...", "retrying portal fetch...", "Portal wird erneut abgerufen..."],
    NoPortalOpenSettings => ["geen portal ingesteld - open de instellingen (o)", "no portal set up - open the settings (o)", "kein Portal eingerichtet - Einstellungen öffnen (o)"],
    BorderlessOn => ["randloos + altijd bovenop  (druk nog eens op t om te herstellen)", "borderless + always on top  (press t again to restore)", "randlos + immer im Vordergrund  (nochmal t zum Wiederherstellen)"],
    BorderlessOff => ["normaal venster", "normal window", "normales Fenster"],
    // News picker
    NoNewsChannels => ["geen nieuwszenders gevonden", "no news channels found", "keine Nachrichtensender gefunden"],
    NoNewsRules => ["geen nieuwsregels ingesteld (instellingen, o)", "no news rules set (settings, o)", "keine Nachrichtenregeln eingestellt (Einstellungen, o)"],
    FetchingNews => ["nieuws ophalen...", "fetching news...", "Nachrichten werden geladen..."],
    NewsTitle => ["Laatste nieuws", "Latest news", "Neueste Nachrichten"],
    NoRecentNews => ["geen recente uitzendingen gevonden", "no recent broadcasts found", "keine aktuellen Sendungen gefunden"],
    NewsFooter => ["pijltjes = kiezen   /   Enter = afspelen   /   Esc = sluiten", "arrows = choose   /   Enter = play   /   Esc = close", "Pfeile = wählen   /   Enter = abspielen   /   Esc = schließen"],
    // Favorites and presets
    NoChannelToFavorite => ["geen zender om als favoriet te bewaren", "no channel to add as favorite", "kein Sender zum Merken"],
    FavoriteRemoved => ["[-] verwijderd: {0}", "[-] removed: {0}", "[-] entfernt: {0}"],
    FavoriteAdded => ["[+] toegevoegd: {0}", "[+] added: {0}", "[+] hinzugefügt: {0}"],
    FavoritesTitle => ["Favorieten", "Favorites", "Favoriten"],
    FavoritesEmpty => ["druk * op een zender om hem toe te voegen", "press * on a channel to add it", "* auf einem Sender drücken, um ihn hinzuzufügen"],
    PresetNoLive => ["sneltoets {0} - geen live-zender actief", "preset {0} - no live channel playing", "Taste {0} - kein Live-Sender aktiv"],
    PresetSaved => ["sneltoets {0} opgeslagen: {1}", "preset {0} saved: {1}", "Taste {0} gespeichert: {1}"],
    PresetEmpty => ["sneltoets {0} is leeg - houd {0} ingedrukt om op te slaan", "preset {0} is empty - hold {0} to save", "Taste {0} ist leer - {0} gedrückt halten zum Speichern"],
    // Search
    SearchHint => ["zoek live / film / serie...", "search live / movie / series...", "Live / Film / Serie suchen..."],
    SeriesNotYet => ["series afspelen kan nog niet: {0}", "playing series is not available yet: {0}", "Serien abspielen ist noch nicht möglich: {0}"],
    // Portal setup and the settings' portal section
    DefaultPortalName => ["Portaal", "Portal", "Portal"],
    DefaultListName => ["M3U-lijst", "M3U list", "M3U-Liste"],
    PortalSetupTitle => ["Portal instellen", "Set up your portal", "Portal einrichten"],
    PortalSetupIntro => ["Vul je Xtream-gegevens in. Ze worden opgeslagen in BongoPlayer.ini naast de app.", "Enter your Xtream details. They are saved in BongoPlayer.ini next to the app.", "Gib deine Xtream-Daten ein. Sie werden in BongoPlayer.ini neben der App gespeichert."],
    ServerLabel => ["Server (host:poort)", "Server (host:port)", "Server (Host:Port)"],
    UserLabel => ["Gebruiker", "Username", "Benutzer"],
    UserHint => ["naam", "name", "Name"],
    PasswordLabel => ["Wachtwoord", "Password", "Passwort"],
    ConnectAndSave => ["Verbinden en opslaan", "Connect and save", "Verbinden und speichern"],
    CannotWrite => ["kon {0} niet schrijven: {1}", "could not write {0}: {1}", "{0} konnte nicht geschrieben werden: {1}"],
    ConnectingPortal => ["verbinden met portal...", "connecting to portal...", "Verbindung zum Portal..."],
    CredsNoAt => ["'@' tussen wachtwoord en server ontbreekt", "missing '@' between password and server", "'@' zwischen Passwort und Server fehlt"],
    CredsNoUserPass => ["':' tussen gebruiker en wachtwoord ontbreekt", "missing ':' between username and password", "':' zwischen Benutzer und Passwort fehlt"],
    CredsBadPort => ["ongeldig poortnummer", "invalid port number", "ungültige Portnummer"],
    TestConnection => ["Verbinding testen", "Test connection", "Verbindung testen"],
    Testing => ["bezig met testen...", "testing...", "wird getestet..."],
    Save => ["Opslaan", "Save", "Speichern"],
    ConnOk => ["Verbonden: {0} zenders", "Connected: {0} channels", "Verbunden: {0} Sender"],
    ConnOkNoCount => ["Verbonden", "Connected", "Verbunden"],
    ConnBadLogin => ["Gebruiker of wachtwoord onjuist", "Wrong username or password", "Benutzer oder Passwort falsch"],
    ConnUnreachable => ["Server niet bereikbaar", "Server not reachable", "Server nicht erreichbar"],
    ConnInvalid => ["Geen geldige portal", "Not a valid portal", "Kein gültiges Portal"],
    PortalFromCommandLine => ["Deze portal komt uit de opdrachtregel (--xtream); wat je hier opslaat geldt pas als je zonder die optie start.", "This portal comes from the command line (--xtream); what you save here only applies when you start without that option.", "Dieses Portal stammt aus der Befehlszeile (--xtream); was du hier speicherst, gilt erst, wenn du ohne diese Option startest."],
    // Empty state and catalog status
    Preparing => ["voorbereiden...", "preparing...", "wird vorbereitet..."],
    Connecting => ["verbinden...", "connecting...", "Verbindung wird hergestellt..."],
    FetchingCatalog => ["catalogus ophalen...", "fetching catalog...", "Katalog wird abgerufen..."],
    CatalogLoadedHint => ["catalogus geladen  -  f = zoeken, g = gids, 0-9 = sneltoetsen, o = instellingen", "catalog loaded  -  f = search, g = guide, 0-9 = presets, o = settings", "Katalog geladen  -  f = Suche, g = Programm, 0-9 = Senderspeicher, o = Einstellungen"],
    PortalFetchFailed => ["portal ophalen mislukt", "portal fetch failed", "Portal-Abruf fehlgeschlagen"],
    PressF5 => ["druk F5 om opnieuw te proberen", "press F5 to retry", "F5 drücken für einen neuen Versuch"],
    EpgTitle => ["Gids", "Guide", "Programm"],
    // Sources: an M3U list
    NoGuideForSource => ["voor deze bron is er nog geen gids", "this source has no guide yet", "für diese Quelle gibt es noch kein Programm"],
    ListNotUpdated => ["zenderlijst niet bijgewerkt", "channel list not updated", "Senderliste nicht aktualisiert"],
    NotAChannelList => ["dit is geen zenderlijst", "this is not a channel list", "das ist keine Senderliste"],
    FetchingList => ["zenderlijst ophalen...", "fetching the channel list...", "Senderliste wird abgerufen..."],
    ListFetchFailed => ["zenderlijst ophalen mislukt", "fetching the channel list failed", "Abruf der Senderliste fehlgeschlagen"],
    SourceSaved => ["bron opgeslagen: {0}", "source saved: {0}", "Quelle gespeichert: {0}"],
    SourceInUse => ["bron in gebruik: {0}", "now using: {0}", "Quelle in Verwendung: {0}"],
    SourceDeleted => ["bron verwijderd: {0}", "source deleted: {0}", "Quelle gelöscht: {0}"],
    // Help
    HelpTitle => ["{0} v{1}  -  sneltoetsen", "{0} v{1}  -  hotkeys", "{0} v{1}  -  Tastenkürzel"],
    HelpFooter => ["(c) 2026 Bart    -    druk ? of Esc om te sluiten", "(c) 2026 Bart    -    press ? or Esc to close", "(c) 2026 Bart    -    ? oder Esc zum Schließen"],
    HelpViewer => ["Kijken (gids dicht)", "Viewer (guide closed)", "Ansicht (Programm geschlossen)"],
    HelpGuide => ["Gids (gids open)", "Guide (guide open)", "Programm (Programm offen)"],
    KeyUpDownWheel => ["Omhoog / Omlaag  -  muiswiel", "Up / Down  -  mouse wheel", "Hoch / Runter  -  Mausrad"],
    HelpZap => ["vorige / volgende zender", "previous / next channel", "vorheriger / nächster Sender"],
    HelpZapSwapped => ["volgende / vorige zender", "next / previous channel", "nächster / vorheriger Sender"],
    KeyDigitsTap => ["0-9  (tik)", "0-9  (tap)", "0-9  (tippen)"],
    HelpPresetRecall => ["sneltoets: zender oproepen", "preset: recall channel", "Senderspeicher abrufen"],
    KeyDigitsHold => ["0-9  (vasthouden)", "0-9  (hold)", "0-9  (halten)"],
    HelpPresetStore => ["huidige zender als sneltoets opslaan", "store current channel as preset", "aktuellen Sender speichern"],
    HelpNews => ["nieuws kiezen (in te stellen bij de instellingen)", "choose news (configurable in the settings)", "Nachrichten wählen (in den Einstellungen anpassbar)"],
    HelpSearch => ["zoeken (live + films + series)", "search (live + movies + series)", "Suche (Live + Filme + Serien)"],
    HelpFavorites => ["favorieten", "favorites", "Favoriten"],
    HelpToggleFavorite => ["huidige zender als favoriet aan/uit", "toggle current channel as favorite", "aktuellen Sender als Favorit an/aus"],
    HelpEpgStrip => ["gidsbalk (nu + straks)", "guide strip (now + next)", "Programmleiste (jetzt + gleich)"],
    HelpEpgGrid => ["gidsoverzicht", "guide overview", "Programmübersicht"],
    HelpAudioSubs => ["audiospoor / ondertiteling wisselen", "next audio track / subtitles", "nächste Tonspur / Untertitel"],
    HelpQuality => ["hogere / lagere kwaliteit (live)", "higher / lower quality (live)", "höhere / niedrigere Qualität (live)"],
    KeyLeftRight => ["Links / Rechts", "Left / Right", "Links / Rechts"],
    HelpSeek => ["30 s terug / vooruit (film, terugkijken)", "back / forward 30 s (movie, catch-up)", "30 s zurück / vor (Film, Wiederholung)"],
    KeySpace => ["Spatie", "Space", "Leertaste"],
    HelpPause => ["pauze / verder (film, serie, terugkijken)", "pause / play (movie, series, catch-up)", "Pause / weiter (Film, Serie, Wiederholung)"],
    HelpFullscreen => ["volledig scherm", "fullscreen", "Vollbild"],
    HelpRetryPortal => ["opnieuw ophalen", "fetch again", "erneut abrufen"],
    HelpDebug => ["debug-info", "debug info", "Debug-Info"],
    HelpOpenGuide => ["gids openen", "open guide", "Programm öffnen"],
    HelpBorderless => ["randloos + altijd bovenop  (aan/uit)", "borderless + always on top  (toggle)", "randlos + immer im Vordergrund  (an/aus)"],
    KeyRightDrag => ["rechtermuisknop + slepen", "right button + drag", "rechte Maustaste + ziehen"],
    HelpRightDrag => ["venster verplaatsen (alleen randloos)", "move the window (borderless only)", "Fenster verschieben (nur randlos)"],
    HelpSettings => ["instellingen", "settings", "Einstellungen"],
    HelpThisHelp => ["deze hulp", "this help", "diese Hilfe"],
    HelpCloseOverlays => ["vensters sluiten", "close overlays", "Fenster schließen"],
    HelpGuideFilter => ["filter: zender of programma", "filter: channel or programme", "Filter: Sender oder Sendung"],
    HelpGuideSearch => ["zoeken in alles: zenders, films, series", "search everything: channels, films, series", "alles durchsuchen: Sender, Filme, Serien"],
    HelpGuideRows => ["programma kiezen in de kolom", "programme cursor in the column", "Sendung in der Spalte wählen"],
    HelpGuideColumns => ["andere kolom", "switch column", "Spalte wechseln"],
    HelpGuidePages => ["vorige / volgende pagina zenders", "previous / next page of channels", "vorherige / nächste Seite der Sender"],
    HelpGuideDate => ["datum kiezen", "choose the date", "Datum wählen"],
    HelpGuideEsc => ["datumlijst sluiten, anders filter wissen, anders gids sluiten", "close the date list, else clear the filter, else close the guide", "Datumsliste schließen, sonst Filter löschen, sonst Programm schließen"],
    HelpGuideHomeEnd => ["eerste / laatste programma", "first / last programme", "erste / letzte Sendung"],
    HelpGuideTimeMode => ["Nu & straks  /  Primetime", "Now & next  /  Primetime", "Jetzt & gleich  /  Primetime"],
    HelpGuidePlay => ["gekozen programma afspelen (live / terugkijken)", "play the selected programme (live / catch-up)", "gewählte Sendung abspielen (live / Wiederholung)"],
    HelpGuideClose => ["gids sluiten", "close guide", "Programm schließen"],
    // The guide of 0.6.0 (part 2)
    GuideCatalogLoading => ["catalogus laadt nog - druk zo nog eens op g", "catalog still loading - press g again in a moment", "Katalog lädt noch - gleich nochmal g drücken"],
    GuideChannelsRange => ["zenders {0}-{1} van {2}", "channels {0}-{1} of {2}", "Sender {0}-{1} von {2}"],
    GuidePlaying => ["speelt: {0}", "playing: {0}", "läuft: {0}"],
    GuideNow => ["Nu & straks", "Now & next", "Jetzt & gleich"],
    GuidePrime => ["Primetime", "Primetime", "Primetime"],
    GuideFilterHint => ["zender of programma", "channel or programme", "Sender oder Sendung"],
    GuideNoChannels => ["geen zenders met programma-informatie", "no channels with programme information", "keine Sender mit Programminformationen"],
    GuideNothingFound => ["niets gevonden voor '{0}'", "nothing found for '{0}'", "nichts gefunden für '{0}'"],
    GuideLiveBadge => ["LIVE", "LIVE", "LIVE"],
    GuideStartsAt => ["begint om {0}", "starts at {0}", "beginnt um {0}"],
    GuideStartsOn => ["begint {0} om {1}", "starts {0} at {1}", "beginnt {0} um {1}"],
    GuideNotReplayable => ["niet terug te kijken", "not available to watch back", "nicht als Wiederholung verfügbar"],
    GuideLiveToast => ["[LIVE] {0} · {1}", "[LIVE] {0} · {1}", "[LIVE] {0} · {1}"],
    GuideKeysFooter => ["Esc of g = sluiten  ·  ? = hulp  ·  pijlen = navigeren  ·  PgUp/PgDn = andere zenders  ·  Enter = afspelen  ·  n / p = nu / primetime  ·  d = datum  ·  / = filter  ·  f = zoeken in alles", "Esc or g = close  ·  ? = help  ·  arrows = navigate  ·  PgUp/PgDn = other channels  ·  Enter = play  ·  n / p = now / primetime  ·  d = date  ·  / = filter  ·  f = search everything", "Esc oder g = schließen  ·  ? = Hilfe  ·  Pfeile = navigieren  ·  PgUp/PgDn = andere Sender  ·  Enter = abspielen  ·  n / p = jetzt / Primetime  ·  d = Datum  ·  / = Filter  ·  f = alles durchsuchen"],
    // Days in the guide
    DayYesterday => ["Gisteren", "Yesterday", "Gestern"],
    DayToday => ["Vandaag", "Today", "Heute"],
    DayTomorrow => ["Morgen", "Tomorrow", "Morgen"],
    // The wait screen while catch-up loads
    WaitLoading => ["programma wordt geladen…", "loading the programme…", "Sendung wird geladen…"],
    WaitFailed => ["Terugkijken lukt niet: {0}", "Catch-up failed: {0}", "Wiederholung nicht möglich: {0}"],
    WaitStreamEnded => ["de stream stopte", "the stream stopped", "der Stream brach ab"],
    WaitEscBack => ["Esc = terug naar de gids", "Esc = back to the guide", "Esc = zurück zum Programm"],
    WaitWhen => ["{0} · {1}–{2}", "{0} · {1}–{2}", "{0} · {1}–{2}"],
    // Debug HUD
    DebugBuffering => ["afspelen: BUFFEREN  {0}s", "playback: BUFFERING  {0}s", "Wiedergabe: PUFFERN  {0}s"],
    DebugBufferingStartup => ["afspelen: bufferen (start)", "playback: buffering (startup)", "Wiedergabe: puffern (Start)"],
    DebugLive => ["afspelen: live", "playback: live", "Wiedergabe: live"],
    DebugFreezes => ["haperingen: {0}   totaal {1}s   laatste {2}", "freezes: {0}   total {1}s   last {2}", "Hänger: {0}   gesamt {1}s   zuletzt {2}"],
    DebugChannel => ["zender: {0}", "channel: {0}", "Sender: {0}"],
    DebugFavs => ["favorieten: {0}", "favs: {0}", "Favoriten: {0}"],
    DebugCatalog => ["catalogus: {0}", "catalog: {0}", "Katalog: {0}"],
    DebugPortal => ["portal: {0}", "portal: {0}", "Portal: {0}"],
    // Settings
    SettingsTitle => ["Instellingen", "Settings", "Einstellungen"],
    SectionLanguage => ["Taal", "Language", "Sprache"],
    SectionPortal => ["Portal", "Portal", "Portal"],
    SectionControls => ["Bediening", "Controls", "Bedienung"],
    SectionAdult => ["Adult-inhoud", "Adult content", "Inhalte für Erwachsene"],
    SectionNews => ["Nieuws (N)", "News (N)", "Nachrichten (N)"],
    SwapZapKeys => ["Zaprichting omdraaien (pijl omhoog = volgende zender)", "Reverse zapping (arrow up = next channel)", "Zapprichtung umkehren (Pfeil hoch = nächster Sender)"],
    SettingsNav => ["Omhoog/Omlaag = onderdeel   /   Enter = openen   /   Tab = volgende   /   Esc = terug", "Up/Down = section   /   Enter = open   /   Tab = next   /   Esc = back", "Hoch/Runter = Bereich   /   Enter = öffnen   /   Tab = weiter   /   Esc = zurück"],
    SaveFailed => ["Instellingen konden niet worden opgeslagen", "Settings could not be saved", "Einstellungen konnten nicht gespeichert werden"],
    AdultHidden => ["Verborgen", "Hidden", "Verborgen"],
    AdultVisible => ["Zichtbaar tot afsluiten", "Visible until you quit", "Sichtbar bis zum Beenden"],
    Unlock => ["Ontgrendelen", "Unlock", "Entsperren"],
    Lock => ["Vergrendelen", "Lock", "Sperren"],
    ChangePin => ["Pincode wijzigen", "Change PIN", "PIN ändern"],
    ForgotPin => ["Pincode vergeten?", "Forgot PIN?", "PIN vergessen?"],
    ChoosePin => ["Kies een pincode (4 cijfers)", "Choose a PIN (4 digits)", "PIN wählen (4 Ziffern)"],
    RepeatPin => ["Herhaal de pincode", "Repeat the PIN", "PIN wiederholen"],
    EnterPin => ["Pincode", "PIN", "PIN"],
    CurrentPin => ["Huidige pincode", "Current PIN", "Aktuelle PIN"],
    NewPin => ["Nieuwe pincode", "New PIN", "Neue PIN"],
    PinFormat => ["De pincode bestaat uit 4 cijfers", "The PIN is 4 digits", "Die PIN besteht aus 4 Ziffern"],
    PinMismatch => ["De pincodes zijn niet gelijk", "The PINs don't match", "Die PINs stimmen nicht überein"],
    PinWrong => ["Onjuiste pincode", "Wrong PIN", "Falsche PIN"],
    PinBlocked => ["Te vaak fout - wacht {0} s", "Too many attempts - wait {0} s", "Zu viele Versuche - {0} s warten"],
    PinChanged => ["Pincode gewijzigd", "PIN changed", "PIN geändert"],
    PortalPassword => ["Portal-wachtwoord", "Portal password", "Portal-Passwort"],
    WrongPortalPassword => ["Onjuist portal-wachtwoord", "Wrong portal password", "Falsches Portal-Passwort"],
    PinCleared => ["Pincode gewist - kies een nieuwe bij het ontgrendelen", "PIN cleared - choose a new one when unlocking", "PIN gelöscht - beim Entsperren eine neue wählen"],
    Unlocked => ["Ontgrendeld tot afsluiten", "Unlocked until you quit", "Entsperrt bis zum Beenden"],
    LockedToast => ["Vergrendeld", "Locked", "Gesperrt"],
    Ok => ["OK", "OK", "OK"],
    Cancel => ["Annuleren", "Cancel", "Abbrechen"],
    NewsColChannel => ["Zender", "Channel", "Sender"],
    NewsColPatterns => ["Titelpatroon(en)", "Title pattern(s)", "Titelmuster"],
    NewsColLabel => ["Label", "Label", "Label"],
    NewsColSkip => ["Begin later (s)", "Start later (s)", "Später beginnen (s)"],
    NewsFound => ["gevonden: {0}", "found: {0}", "gefunden: {0}"],
    NewsNotFound => ["niet gevonden", "not found", "nicht gefunden"],
    NewsAddRule => ["Regel toevoegen", "Add rule", "Regel hinzufügen"],
    NewsRemove => ["Verwijderen", "Remove", "Entfernen"],
    NewsRestoreDefaults => ["Standaard herstellen", "Restore defaults", "Standard wiederherstellen"],
    NewsPatternsHelp => ["* = willekeurige tekst   ;   ! = uitsluiten   ;   ; = scheiding", "* = any text   ;   ! = exclude   ;   ; = separator", "* = beliebiger Text   ;   ! = ausschließen   ;   ; = Trennung"],
}

/// The text for `key` in `lang`.
pub fn t(lang: Lang, key: T) -> &'static str {
    let [nl, en, de] = table(key);
    match lang {
        Lang::Nl => nl,
        Lang::En => en,
        Lang::De => de,
    }
}

/// The text for `key` with `{0}`, `{1}`, … filled from `args`; each language
/// may put them in its own order.
pub fn tr(lang: Lang, key: T, args: &[&str]) -> String {
    let template = t(lang, key);
    let mut result = String::with_capacity(template.len() * 2);
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            // Try to parse a placeholder {N}
            let mut num_str = String::new();
            let mut is_placeholder = false;

            // Collect digits
            while let Some(&next_ch) = chars.peek() {
                if next_ch.is_ascii_digit() {
                    num_str.push(chars.next().unwrap());
                } else {
                    break;
                }
            }

            // Check if we have a closing }
            if !num_str.is_empty() && chars.peek() == Some(&'}') {
                chars.next(); // consume }
                is_placeholder = true;
            }

            if is_placeholder {
                // Parse the index and substitute if available
                if let Ok(idx) = num_str.parse::<usize>() {
                    if idx < args.len() {
                        result.push_str(args[idx]);
                    } else {
                        // Missing arg: keep the placeholder
                        result.push('{');
                        result.push_str(&num_str);
                        result.push('}');
                    }
                } else {
                    // Invalid number: keep as-is
                    result.push('{');
                    result.push_str(&num_str);
                    result.push('}');
                }
            } else {
                // Not a valid placeholder: keep the { and any digits we collected
                result.push('{');
                result.push_str(&num_str);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Weekdays (Monday first) and months, short, per language (NL, EN, DE).
const WEEKDAYS: [[&str; 7]; 3] = [
    ["ma", "di", "wo", "do", "vr", "za", "zo"],
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
];
const MONTHS: [[&str; 12]; 3] = [
    [
        "jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec",
    ],
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
];

/// How the guide names `day`: "Gisteren", "Vandaag", "Morgen", else weekday,
/// day and month ("do 8 okt", "Thu 8 Oct", "Do. 8. Okt.").
pub fn day_label(lang: Lang, day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    match (day - today).num_days() {
        -1 => t(lang, T::DayYesterday).to_string(),
        0 => t(lang, T::DayToday).to_string(),
        1 => t(lang, T::DayTomorrow).to_string(),
        _ => {
            let row = match lang {
                Lang::Nl => 0,
                Lang::En => 1,
                Lang::De => 2,
            };
            let weekday = WEEKDAYS[row][day.weekday().num_days_from_monday() as usize];
            let month = MONTHS[row][day.month0() as usize];
            match lang {
                Lang::De => format!("{weekday}. {}. {month}.", day.day()),
                _ => format!("{weekday} {} {month}", day.day()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(s: &str) -> Vec<u32> {
        (0..10)
            .filter(|i| s.contains(&format!("{{{i}}}")))
            .collect()
    }

    #[test]
    fn every_translation_keeps_the_same_placeholders() {
        for &key in T::ALL {
            let [nl, en, de] = table(key);
            assert_eq!(marks(nl), marks(en), "{key:?}");
            assert_eq!(marks(nl), marks(de), "{key:?}");
        }
    }

    #[test]
    fn no_text_is_empty() {
        for &key in T::ALL {
            for l in Lang::ALL {
                assert!(!t(l, key).trim().is_empty(), "{key:?} in {l:?}");
            }
        }
    }

    #[test]
    fn tr_fills_placeholders() {
        assert_eq!(
            tr(Lang::En, T::PresetSaved, &["3", "NPO 1"]),
            "preset 3 saved: NPO 1"
        );
        assert_eq!(
            tr(Lang::Nl, T::PresetEmpty, &["5"]),
            "sneltoets 5 is leeg - houd 5 ingedrukt om op te slaan"
        );
        assert_eq!(
            tr(Lang::De, T::GuideChannelsRange, &["1", "5", "40"]),
            "Sender 1-5 von 40"
        );
    }

    #[test]
    fn tr_does_not_re_expand_placeholders_in_values() {
        // A value containing {1} should not be expanded again
        assert_eq!(
            tr(Lang::En, T::VariantUnavailable, &["Movie {1}", "FHD"]),
            "[TV] Movie {1} (FHD not available)"
        );
        // Same for Dutch
        assert_eq!(
            tr(Lang::Nl, T::VariantUnavailable, &["Tatort {1}", "FHD"]),
            "[TV] Tatort {1} (FHD niet beschikbaar)"
        );
    }

    #[test]
    fn tr_leaves_missing_args_as_placeholders() {
        // Missing arg {1} should stay as {1}
        assert_eq!(tr(Lang::En, T::PresetSaved, &["3"]), "preset 3 saved: {1}");
    }

    #[test]
    fn tr_ignores_extra_args() {
        // Extra args are ignored
        assert_eq!(
            tr(Lang::En, T::PresetEmpty, &["5", "x"]),
            "preset 5 is empty - hold 5 to save"
        );
    }

    #[test]
    fn language_codes_round_trip() {
        for l in Lang::ALL {
            assert_eq!(Lang::from_code(l.code()), Some(l));
        }
        assert_eq!(Lang::from_code(" NL "), Some(Lang::Nl));
        assert_eq!(Lang::from_code("fr"), None);
    }

    #[test]
    fn the_windows_language_picks_dutch_german_or_english() {
        assert_eq!(lang_from_langid(0x0413), Lang::Nl); // nl-NL
        assert_eq!(lang_from_langid(0x0813), Lang::Nl); // nl-BE
        assert_eq!(lang_from_langid(0x0407), Lang::De); // de-DE
        assert_eq!(lang_from_langid(0x0C07), Lang::De); // de-AT
        assert_eq!(lang_from_langid(0x0409), Lang::En); // en-US
        assert_eq!(lang_from_langid(0x040C), Lang::En); // fr-FR
    }

    #[test]
    fn days_are_named_like_a_tv_guide() {
        // 12 October 2026 is a Monday.
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let d = |n: i64| today + chrono::Duration::days(n);
        assert_eq!(day_label(Lang::Nl, d(-1), today), "Gisteren");
        assert_eq!(day_label(Lang::En, d(0), today), "Today");
        assert_eq!(day_label(Lang::De, d(1), today), "Morgen");
        assert_eq!(day_label(Lang::Nl, d(-4), today), "do 8 okt");
        assert_eq!(day_label(Lang::En, d(-4), today), "Thu 8 Oct");
        assert_eq!(day_label(Lang::De, d(-4), today), "Do. 8. Okt.");
        assert_eq!(day_label(Lang::Nl, d(3), today), "do 15 okt");
    }
}
