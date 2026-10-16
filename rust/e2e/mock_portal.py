"""Mock Xtream portal for Bongo IPTV Player end-to-end tests.

    py -3 mock_portal.py PORT SEGDIR LOGFILE [MODE]

player_api.php   login check (user "tester", password "secret"), live / VOD /
                 series lists, EPG
/live/u/p/N.m3u8 302 to /edge/tokN/1: a fresh token per request, like the real
                 portal's edge redirect. The edge serves a sliding live
                 playlist of 4 s segments.
/timeshift/...   302 to /tsplay/vod.m3u8 (catch-up; generating it takes 2.5 s)
/seg/TOK/N.ts    the test segments made by make-segments.ps1

MODE: none (default) | once | always - the first (once) or every (always)
edge token answers 509 from its first reload on.

Environment:
  MOCK_CHANNELS=N    live channels "TEST 01" .. "TEST NN" (default 40)
  MOCK_VARIANTS=1    channel 05 also as "TEST 05 UHD" (105) and "TEST 05 FHD" (205)
  MOCK_ADULT=1       after TEST 02: "24/7 | ADULT A" (901, is_adult=1) and
                     "XXX | ADULT B" (902, is_adult=1; the XXX in its name means
                     nothing, the flag does); VOD "Adult Film" (9001, is_adult=1)
                     and "Gewone Film" (9002)
  MOCK_XXX=1         names that start with XXX but carry no flag, so they are
                     ordinary content: the live channel "XXX Something" (903, last)
                     and the VOD film "xXx: State of the Union" (9003)
  MOCK_NEWS=1        "NL | NPO 1" (501) with archive twin "NL | NPO1 HD" (601,
                     tv_archive=1); the twin's EPG holds three past programmes:
                     NOS Journaal, NOS Jeugdjournaal, Tussen Kunst en Kitsch
  MOCK_DENY=105,...  streams refused to the account (HTTP 407)
  MOCK_BUSY_UNTIL=S  live requests answer 503 for the first S seconds
  MOCK_LATENCY=1     answer about as slowly as the real portal
  MOCK_LOGIN_STATUS=N  the login (player_api.php without an action) answers HTTP N,
                     e.g. 503 or 509, the real portal's busy answers
  MOCK_EPG_DELAY=S   day-EPG requests (the news fetch) answer after S seconds, so
                     a test can press n again while a fetch is in flight
  MOCK_GUIDE=1       ten channels "NL | GIDS k" (701..710) with catch-up twins
                     "NL | GIDSk HD" (801..810, 7 days): logos /logo/k.png (a GIF for
                     9, none for 10) and EPG from a week back to three days ahead,
                     "Journaal k" at 19:00 local every day; the programme on now comes
                     twice (5 minutes apart) with an overlapping one at its end, as the
                     real portal sends it; channel 2 sends no has_archive
  MOCK_M3U=1         an M3U list of 21 channels at /m3u/list.m3u: "M3U 01".."M3U 18" (tvg-id
                     m3uk.test) play /m3uplay/k.m3u8, a live playlist on the test segments
                     with no token; "M3U UA" (/m3uplay/ua.m3u8) plays only with the
                     User-Agent BongoTest/1.0, which its #EXTVLCOPT line names (403
                     otherwise); "M3U DOOD" (/m3uplay/dead.m3u8) is 404; "M3U REF"
                     (/m3uplay/ref.m3u8, last) plays only with the Referer http://ref.test/,
                     which its #EXTVLCOPT line names (403 otherwise). Any other playable
                     /m3uplay/ request that carries a Referer header answers 400: a
                     referrer that leaked from an earlier load. (M3U DOOD and
                     /m3uplay/none/... are not playable: they answer 404 first.)
                     M3U DOOD's address is under localhost, every other one under
                     127.0.0.1: the check before a defect mark never asks the server of
                     the stream that plays, and localhost is another server by its host
                     that still reaches this mock. The mock listens on 127.0.0.1 only;
                     Windows resolves localhost to ::1 and then 127.0.0.1, and the ::1
                     attempt is refused, so mpv and the check go on to 127.0.0.1.
  MOCK_M3U_BIG=N     /m3u/big.m3u: N channels "GROOT nnnnn" in iptv-org's form (logos, groups,
                     an x-tvg-url); the first 18 play like M3U 01..18 (the startup measurement)
  MOCK_VOD_BIG=1     a film and series catalog of Pandora's size, as the portal sends it (every
                     value a string): 47,188 films (one in ten another version of the one before),
                     10,589 series, 200 film and 100 series categories. Made once (about half a
                     second) and kept in the temp folder (bongo-mock-vod-big-1), so a mock that
                     starts reads them before it answers. It takes the place of MOCK_ADULT's and
                     MOCK_XXX's films (measure-startup.ps1 -VodBig)
"""
import base64
import datetime
import http.server
import json
import os
import struct
import sys
import tempfile
import threading
import time
import urllib.parse
import zlib

PORT, SEGDIR, LOG = int(sys.argv[1]), sys.argv[2], sys.argv[3]
MODE = sys.argv[4] if len(sys.argv) > 4 else "none"
ENV = os.environ.get
LATENCY = ENV("MOCK_LATENCY") == "1"
SEG_DUR = 4
NSEG = len([f for f in os.listdir(SEGDIR) if f.endswith(".ts")])
T0 = time.time()
lock = threading.Lock()
edge_hits, dead = {}, set()
state = {"tokens": 0, "failed_once": False}


def log(code, path):
    with lock, open(LOG, "a") as f:
        f.write(f"{time.time() - T0:7.2f}s  {code}  {path}\n")


def b64(s):
    return base64.b64encode(s.encode()).decode()


def live_streams():
    n = int(ENV("MOCK_CHANNELS", "40"))

    def ch(sid, name, archive=0, adult=0):
        return {"stream_id": sid, "name": name, "category_id": "1", "epg_channel_id": "",
                "tv_archive": archive, "is_adult": str(adult)}

    out = []
    for i in range(1, n + 1):
        out.append(ch(i, f"TEST {i:02d}"))
        if i == 2 and ENV("MOCK_ADULT") == "1":
            out += [ch(901, "24/7 | ADULT A", adult=1), ch(902, "XXX | ADULT B", adult=1)]
    if ENV("MOCK_VARIANTS") == "1":
        out += [ch(105, "TEST 05 UHD"), ch(205, "TEST 05 FHD")]
    if ENV("MOCK_NEWS") == "1":
        out += [ch(501, "NL | NPO 1"), ch(601, "NL | NPO1 HD", archive=1)]
    if ENV("MOCK_XXX") == "1":
        out += [ch(903, "XXX Something")]
    if ENV("MOCK_GUIDE") == "1":
        out += guide_streams()
    return out


def vod_streams():
    films = []
    if ENV("MOCK_ADULT") == "1":
        films += [{"stream_id": 9001, "name": "Adult Film", "container_extension": "mp4", "is_adult": "1"},
                  {"stream_id": 9002, "name": "Gewone Film", "container_extension": "mp4", "is_adult": "0"}]
    if ENV("MOCK_XXX") == "1":
        films += [{"stream_id": 9003, "name": "xXx: State of the Union", "container_extension": "mp4",
                   "is_adult": "0"}]
    return films


WORDS = ["the", "last", "night", "of", "dark", "matrix", "love", "story", "dead", "city", "secret",
         "man", "woman", "king", "lost", "house", "blood", "river", "star", "war", "pokémon", "zomer",
         "liefde", "nacht"]
VOD_BIG_ACTIONS = ["get_vod_streams", "get_series", "get_vod_categories", "get_series_categories"]


def mix(i):
    """A number from i, the same on every run (as tests/test_vod_timing.rs has it)."""
    m64 = (1 << 64) - 1
    x = (i * 0x9E3779B97F4A7C15) & m64
    x ^= x >> 29
    return ((x * 0xBF58476D1CE4E5B9) & m64) >> 32


def big_title(i):
    t = " ".join(WORDS[mix(i * 7 + k) % len(WORDS)] for k in range(2 + mix(i) % 3))
    return f"{t[:1].upper()}{t[1:]} {i % 997}"


def big_cover(m):
    # A cover for nine titles in ten; m comes from mix.
    return f"https://image.tmdb.org/t/p/w600_and_h900_bestv2/{m:027x}.jpg" if m % 10 else ""


def make_vod_big():
    """MOCK_VOD_BIG's lists as JSON bytes, by action."""
    films_n = 47188
    prefixes = ["", "NL | ", "EN | ", "4K | "]
    labels = ["", " 1080p", " 4K", " NL SUBS", " [NL]", " HD"]
    films = []
    for i in range(films_n):
        base = i - 1 if i % 10 == 9 else i
        m, mb = mix(i), mix(base)
        r = m % 101
        name = f"{prefixes[mb % 4]}{big_title(base)} ({1950 + mb % 77}){labels[mix(i + 3) % 6]}"
        films.append({"num": i + 1, "name": name, "stream_type": "movie", "stream_id": str(100000 + i),
                      "stream_icon": big_cover(m), "rating": f"{r / 10:.1f}", "rating_5based": r / 20,
                      "added": str(1700000000 + m % 90000000), "is_adult": "1" if i % 97 == 0 else "0",
                      "category_id": str(1 + m % 200), "container_extension": ("mkv", "mp4")[i % 2],
                      "custom_sid": "", "direct_source": ""})
    genres = ["Drama", "Comedy", "Crime, Drama", "Sci-Fi & Fantasy", "Kids", "Reality", ""]
    series = []
    for i in range(10589):
        m = mix(i)
        r = mix(i + 11) % 101
        c = big_cover(mix(films_n + i))
        series.append({"num": i + 1, "name": big_title(films_n + i), "series_id": 1 + i, "cover": c,
                       "plot": "A family's secrets come out when a child goes missing in a small town.",
                       "cast": "An Actor, Another Actor, A Third Actor", "director": "A Director",
                       "genre": genres[m % 7], "releaseDate": f"{1990 + m % 36}-05-01",
                       "last_modified": str(1700000000 + m % 90000000), "rating": f"{r / 10:.1f}",
                       "rating_5based": r / 20, "backdrop_path": [c] if c else [], "youtube_trailer": "",
                       "episode_run_time": "45", "category_id": str(1 + m % 100),
                       "is_adult": "1" if i % 89 == 0 else "0"})
    names = ["NL | Komedie", "EN | Action", "NL | Kids", "4K Films", "Nieuw", "EN | Drama", "Horror",
             "Documentaire"]

    def categories(n):
        return [{"category_id": str(1 + i), "category_name": f"{names[i % 8]} {i}", "parent_id": 0}
                for i in range(n)]

    lists = [films, series, categories(200), categories(100)]
    return {a: json.dumps(l, separators=(",", ":")).encode() for a, l in zip(VOD_BIG_ACTIONS, lists)}


def vod_big():
    """MOCK_VOD_BIG's lists, read from the temp folder; made and kept there the first
    time, so no later start of the mock pays for them."""
    folder = os.path.join(tempfile.gettempdir(), "bongo-mock-vod-big-1")
    try:
        lists = {}
        for a in VOD_BIG_ACTIONS:
            with open(os.path.join(folder, a + ".json"), "rb") as f:
                lists[a] = f.read()
        return lists
    except OSError:
        pass
    lists = make_vod_big()
    os.makedirs(folder, exist_ok=True)
    for a, body in lists.items():
        with open(os.path.join(folder, a + ".json.tmp"), "wb") as f:
            f.write(body)
        os.replace(os.path.join(folder, a + ".json.tmp"), os.path.join(folder, a + ".json"))
    return lists


VOD_BIG = vod_big() if ENV("MOCK_VOD_BIG") == "1" else {}


def news_epg():
    now = int(time.time())
    progs = [("NOS Journaal", now - 7200, now - 6600),
             ("NOS Jeugdjournaal", now - 3600, now - 3000),
             ("Tussen Kunst en Kitsch", now - 2400, now + 1200)]
    fmt = lambda t: time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(t))
    return [{"id": str(i), "epg_id": str(i), "title": b64(t), "lang": "", "start": fmt(s), "end": fmt(e),
             "description": b64(""), "channel_id": "npo1", "start_timestamp": str(s),
             "stop_timestamp": str(e), "now_playing": 0, "has_archive": 1}
            for i, (t, s, e) in enumerate(progs, 1)]


GUIDE_N = 10


def guide_streams():
    """MOCK_GUIDE's channels: live and catch-up twin per channel, logos."""
    out = []
    for k in range(1, GUIDE_N + 1):
        if k <= 8:
            logo = f"http://127.0.0.1:{PORT}/logo/{k}.png"
        elif k == 9:
            logo = f"http://127.0.0.1:{PORT}/logo/{k}.gif"
        else:
            logo = ""
        out.append({"stream_id": 700 + k, "name": f"NL | GIDS {k}", "category_id": "2",
                    "epg_channel_id": "", "tv_archive": 0, "is_adult": "0", "stream_icon": logo})
        out.append({"stream_id": 800 + k, "name": f"NL | GIDS{k} HD", "category_id": "2",
                    "epg_channel_id": "", "tv_archive": 1, "tv_archive_duration": 7,
                    "is_adult": "0", "stream_icon": ""})
    return out


def guide_epg(k):
    """Hour-long programmes from local midnight a week ago to three days ahead,
    "Journaal k" at 19:00. The programme on now twice and an overlap at its end;
    has_archive 1 for the past week (0 before it and ahead), none on channel 2.
    The hours are counted on the wall clock, so the second pass of the hour the
    clocks go back (the last Sunday of October, 02:00-03:00) has no programme:
    then nothing is on now, and nothing comes twice."""
    now = time.time()
    first = datetime.datetime.combine(datetime.date.today() - datetime.timedelta(days=7),
                                      datetime.time())
    progs = []
    for h in range(24 * 10):
        at = first + datetime.timedelta(hours=h)
        title = f"Journaal {k}" if at.hour == 19 else f"Programma {k}-{h}"
        progs.append((title, at.timestamp(), at.timestamp() + 3600))
    on_now = next((p for p in progs if p[1] <= now < p[2]), None)
    if on_now:
        progs.append((on_now[0], on_now[1] + 300, on_now[2] + 300))
        progs.append((f"Overlap {k}", on_now[2] - 600, on_now[2] + 1200))
    fmt = lambda t: time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(t))
    out = []
    for i, (t, s, e) in enumerate(progs, 1):
        item = {"id": str(i), "epg_id": str(i), "title": b64(t), "lang": "",
                "start": fmt(s), "end": fmt(e), "description": b64(""),
                "channel_id": f"gids{k}", "start_timestamp": str(int(s)),
                "stop_timestamp": str(int(e)), "now_playing": 0}
        if k != 2:
            item["has_archive"] = 1 if now - 7 * 86400 <= s and e <= now else 0
        out.append(item)
    return out


def png(k):
    """A 96x36 logo in a colour of its own."""
    w, h = 96, 36
    rgb = bytes([(40 * k) % 256, (90 + 30 * k) % 256, (200 - 15 * k) % 256])
    raw = b"".join(b"\x00" + rgb * w for _ in range(h))

    def chunk(kind, data):
        crc = zlib.crc32(kind + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", crc)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


M3U_UA = "BongoTest/1.0"
M3U_REF = "http://ref.test/"


def m3u_list():
    """MOCK_M3U's list: 21 channels."""
    lines = ["#EXTM3U"]
    for k in range(1, 19):
        lines += [f'#EXTINF:-1 tvg-id="m3u{k}.test" group-title="Test",M3U {k:02d}',
                  f"http://127.0.0.1:{PORT}/m3uplay/{k}.m3u8"]
    lines += ['#EXTINF:-1 group-title="Test",M3U UA', f"#EXTVLCOPT:http-user-agent={M3U_UA}",
              f"http://127.0.0.1:{PORT}/m3uplay/ua.m3u8"]
    # Under localhost, every other channel under 127.0.0.1: see MOCK_M3U above.
    lines += ['#EXTINF:-1 group-title="Test",M3U DOOD', f"http://localhost:{PORT}/m3uplay/dead.m3u8"]
    # Last, so that no earlier channel moves.
    lines += ['#EXTINF:-1 group-title="Test",M3U REF', f"#EXTVLCOPT:http-referrer={M3U_REF}",
              f"http://127.0.0.1:{PORT}/m3uplay/ref.m3u8"]
    return "\n".join(lines) + "\n"


def m3u_big(n):
    """MOCK_M3U_BIG's list of n channels."""
    lines = ['#EXTM3U x-tvg-url="https://epg.example/guide.xml.gz"']
    for i in range(n):
        play = f"http://127.0.0.1:{PORT}/m3uplay/{i + 1}.m3u8" if i < 18 else \
            f"http://127.0.0.1:{PORT}/m3uplay/none/{i}.m3u8"
        lines += [f'#EXTINF:-1 tvg-id="Groot{i}.xx" tvg-logo="https://logos.example/{i}.png" '
                  f'group-title="Groep {i % 183}",GROOT {i:05d}', play]
    return "\n".join(lines) + "\n"


def live_playlist(tag):
    """A sliding live playlist of 4 s segments under /seg/<tag>/."""
    first = max(0, int((time.time() - T0) / SEG_DUR) - 2)
    lines = ["#EXTM3U", "#EXT-X-VERSION:3", f"#EXT-X-TARGETDURATION:{SEG_DUR}",
             f"#EXT-X-MEDIA-SEQUENCE:{first}"]
    for s in range(first, first + 3):
        lines += [f"#EXTINF:{SEG_DUR}.000,", f"/seg/{tag}/{s}.ts"]
    return ("\n".join(lines) + "\n").encode()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def reply(self, code, body=b"", ctype="text/plain", headers=None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)
        log(code, self.path)

    def json(self, obj):
        return self.reply(200, json.dumps(obj).encode(), "application/json")

    def do_GET(self):
        p = self.path
        if LATENCY:
            time.sleep(0.30 if p.startswith("/live/") else 0.12 if p.startswith("/edge/") else 0.05)
        if p.startswith("/player_api.php"):
            q = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(p).query))
            action = q.get("action")
            if action is None:
                if ENV("MOCK_LOGIN_STATUS"):
                    return self.reply(int(ENV("MOCK_LOGIN_STATUS")), b"busy")
                ok = q.get("username") == "tester" and q.get("password") == "secret"
                return self.json({"user_info": {"auth": 1 if ok else 0, "status": "Active"}, "server_info": {}})
            if action in VOD_BIG:
                return self.reply(200, VOD_BIG[action], "application/json")
            if action == "get_live_streams":
                return self.json(live_streams())
            if action == "get_vod_streams":
                return self.json(vod_streams())
            if action == "get_simple_data_table" and ENV("MOCK_EPG_DELAY"):
                time.sleep(float(ENV("MOCK_EPG_DELAY")))
            if action in ("get_simple_data_table", "get_short_epg"):
                sid = q.get("stream_id", "")
                if ENV("MOCK_GUIDE") == "1" and sid.isdigit() and 801 <= int(sid) <= 800 + GUIDE_N:
                    return self.json({"epg_listings": guide_epg(int(sid) - 800)})
                if q.get("stream_id") == "601" and ENV("MOCK_NEWS") == "1":
                    return self.json({"epg_listings": news_epg()})
                return self.json({"epg_listings": []})
            return self.json([])
        if p.startswith("/timeshift/"):
            return self.reply(302, headers={"Location": f"http://127.0.0.1:{PORT}/tsplay/vod.m3u8"})
        if p.startswith("/tsplay/"):
            time.sleep(2.5)
            lines = ["#EXTM3U", "#EXT-X-VERSION:3", f"#EXT-X-TARGETDURATION:{SEG_DUR}",
                     "#EXT-X-MEDIA-SEQUENCE:0", "#EXT-X-PLAYLIST-TYPE:VOD"]
            for s in range(NSEG):
                lines += [f"#EXTINF:{SEG_DUR}.000,", f"/seg/vod/{s}.ts"]
            lines.append("#EXT-X-ENDLIST")
            return self.reply(200, ("\n".join(lines) + "\n").encode(), "application/vnd.apple.mpegurl")
        if p.startswith("/live/") and p.endswith(".m3u8"):
            sid = p.rsplit("/", 1)[1].split(".")[0]
            if sid in ENV("MOCK_DENY", "").split(","):
                return self.reply(407)
            if time.time() - T0 < float(ENV("MOCK_BUSY_UNTIL", "0")):
                return self.reply(503, b"Service Unavailable")
            with lock:
                state["tokens"] += 1
                tok = state["tokens"]
            return self.reply(302, headers={"Location": f"http://127.0.0.1:{PORT}/edge/tok{tok}/1"})
        if p.startswith("/edge/"):
            tok = p.split("/")[2]
            with lock:
                edge_hits[tok] = edge_hits.get(tok, 0) + 1
                if edge_hits[tok] == 2 and (MODE == "always" or (MODE == "once" and not state["failed_once"])):
                    dead.add(tok)
                    state["failed_once"] = True
                is_dead = tok in dead
            if is_dead:
                return self.reply(509, b"Bandwidth Limit Exceeded")
            first = max(0, int((time.time() - T0) / SEG_DUR) - 2)
            lines = ["#EXTM3U", "#EXT-X-VERSION:3", f"#EXT-X-TARGETDURATION:{SEG_DUR}",
                     f"#EXT-X-MEDIA-SEQUENCE:{first}"]
            for s in range(first, first + 3):
                lines += [f"#EXTINF:{SEG_DUR}.000,", f"/seg/{tok}/{s}.ts"]
            return self.reply(200, ("\n".join(lines) + "\n").encode(), "application/vnd.apple.mpegurl")
        if p.split("?")[0] == "/m3u/list.m3u" and ENV("MOCK_M3U") == "1":
            return self.reply(200, m3u_list().encode(), "audio/x-mpegurl")
        if p.split("?")[0] == "/m3u/big.m3u" and ENV("MOCK_M3U_BIG"):
            return self.reply(200, m3u_big(int(ENV("MOCK_M3U_BIG"))).encode(), "audio/x-mpegurl")
        if p.startswith("/m3uplay/"):
            name = p.rsplit("/", 1)[1].split(".")[0]
            if name == "dead" or "/none/" in p:
                return self.reply(404, b"not found")
            # Only M3U REF asks for a Referer. One on any other channel leaked from an
            # earlier load (mpv's per-file `referrer` once came back as an empty string
            # and went out with every load after it); an empty one counts as well.
            referer = self.headers.get("Referer")
            if name == "ref":
                if referer != M3U_REF:
                    return self.reply(403, b"forbidden")
            elif referer is not None:
                return self.reply(400, b"a Referer that this channel did not ask for")
            if name == "ua" and self.headers.get("User-Agent") != M3U_UA:
                return self.reply(403, b"forbidden")
            return self.reply(200, live_playlist(f"m3u{name}"), "application/vnd.apple.mpegurl")
        if p.startswith("/seg/"):
            n = int(p.rsplit("/", 1)[1].split(".")[0])
            with open(os.path.join(SEGDIR, f"seg{n % NSEG}.ts"), "rb") as f:
                return self.reply(200, f.read(), "video/mp2t")
        if p.startswith("/logo/"):
            name = p.rsplit("/", 1)[1]
            if name.endswith(".png"):
                return self.reply(200, png(int(name.split(".")[0])), "image/png")
            return self.reply(200, b"GIF89a\x01\x00\x01\x00", "image/gif")
        self.reply(404, b"not found")

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
