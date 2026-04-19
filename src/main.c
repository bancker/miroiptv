#include "render.h"
#include "player.h"
#include "npo.h"
#include "sync.h"
#include "xtream.h"
#include <SDL2/SDL.h>
#include <curl/curl.h>
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* Build the URL for channel index 0..2 using these precedence rules:
 *   1. if xtream portal configured -> xtream stream URL for that channel
 *   2. if argv[1] direct URL given -> that URL (same URL for all channels; restart on switch)
 *   3. otherwise NULL -> playback_open falls through to the (broken) NPO resolver
 * Returned string is malloc'd and caller frees, OR NULL if no source. */
static char *build_channel_url(int ch_idx, const xtream_t *portal, const char *direct_url) {
    if (portal && portal->host) return xtream_stream_url(portal, XTREAM_NPO_STREAM_IDS[ch_idx]);
    if (direct_url)             return strdup(direct_url);
    return NULL;
}

typedef struct {
    player_t     player;
    audio_out_t  audio;
    video_tex_t  tex;
    av_clock_t   clk;
    epg_t        epg;
    char        *url;
    const npo_channel_t *channel;
    /* stream_id: operational identity of the currently playing stream.
     *   0  = NPO mode. Restart URL derived from pb->channel via XTREAM_NPO_STREAM_IDS.
     *   >0 = Xtream portal mode (NPO or zap). Restart uses this id directly;
     *        switch comparisons distinguish this playback from pb->channel alone.
     * Without this field, pb->channel stays "sticky" after a wheel-zap so
     * 1/2/3, 'n', and stall-restart all treated the prior NPO channel as the
     * "current" one even while the user was watching TROS Radar. */
    int          stream_id;

    /* Timeshift state. timeshift_start != 0 means we're playing a
     * /timeshift/.../START/STREAM.ts URL; left/right arrow shift the start
     * by TIMESHIFT_STEP_SEC and reopen. stream_id is the archive id and
     * timeshift_dur_min is the URL's duration segment. */
    time_t       timeshift_start;
    int          timeshift_dur_min;
} playback_t;

/* epg_stream_id:
 *   0  -> derive from ch (existing NPO 1/2/3 behavior via XTREAM_NPO_STREAM_IDS)
 *   -1 -> skip EPG fetch (used when zapping to a non-NPO portal channel)
 *   >0 -> use this exact stream_id for the portal's get_short_epg call. */
static int playback_open(playback_t *pb, render_t *r, const npo_channel_t *ch,
                         const char *override_url, const xtream_t *portal,
                         int epg_stream_id) {
    /* Every playback_open is a significant event — log it so we can see which
     * path triggered each reopen from the diagnostic log (initial boot vs zap
     * worker vs stall restart vs channel switch vs arrow seek vs news handler). */
    fprintf(stderr, "[playback_open] ch=%s epg_id=%d url=%.80s\n",
            ch ? ch->display : "(null)", epg_stream_id,
            override_url ? override_url : "(derived)");
    memset(pb, 0, sizeof(*pb));
    pb->channel   = ch;
    /* Persist the stream identity (used later by stall-restart and the
     * switch comparisons). 0 means "NPO mode, derive from ch". */
    pb->stream_id = epg_stream_id > 0 ? epg_stream_id : 0;

    if (override_url) {
        pb->url = strdup(override_url);
    } else if (npo_resolve_stream(ch, &pb->url) != 0) {
        fprintf(stderr, "playback_open: resolve failed for %s (NPO API broken, see R1)\n",
                ch->display);
        return -1;
    }

    if (player_open(&pb->player, pb->url) != 0) { free(pb->url); pb->url = NULL; return -1; }

    /* EPG: prefer portal's get_short_epg when running in xtream mode (it works
     * today); fall back to NPO's start-api when no portal (currently broken,
     * R1). Either failure is non-fatal — overlay shows "(geen programma)". */
    int epg_rc = -1;
    if (epg_stream_id == -1) {
        /* caller said: skip EPG entirely. */
    } else if (portal && portal->host) {
        int use_id = epg_stream_id > 0
                   ? epg_stream_id
                   : XTREAM_NPO_STREAM_IDS[(int)(ch - &NPO_CHANNELS[0])];
        epg_rc = xtream_fetch_epg(portal, use_id, &pb->epg);
        if (epg_rc == 0) {
            fprintf(stderr, "EPG: loaded %zu entries from portal for %s\n",
                    pb->epg.count, ch->display);
        }
    } else {
        epg_rc = npo_fetch_epg(ch, &pb->epg);
    }
    if (epg_rc != 0 && epg_stream_id != -1) {
        fprintf(stderr, "warn: EPG fetch failed for %s\n", ch->display);
        memset(&pb->epg, 0, sizeof(pb->epg));
    }

    if (player_start(&pb->player) != 0) {
        npo_epg_free(&pb->epg); player_close(&pb->player);
        free(pb->url); pb->url = NULL; return -1;
    }
    if (audio_open(&pb->audio, &pb->player.audio_q, pb->player.audio_sample_rate_out) != 0) {
        npo_epg_free(&pb->epg); player_stop(&pb->player); player_close(&pb->player);
        free(pb->url); pb->url = NULL; return -1;
    }

    av_clock_init_from_audio(&pb->clk,
                             &pb->audio.samples_played,
                             &pb->audio.first_pts,
                             &pb->audio.has_first_pts,
                             pb->audio.sample_rate);
    video_tex_init(&pb->tex);
    /* NB: window title is set by the caller on the main thread. We used to
     * SDL_SetWindowTitle here, but when playback_open is called from the zap
     * worker that SDL call happens off-main, which SDL docs forbid and which
     * could crash on some platforms. (void)r; kept to retain the signature. */
    (void)r;
    return 0;
}

/* Small helper so callers don't repeat the "tv - <display>" format. Must be
 * called from the main thread. */
static void set_window_title(render_t *r, const char *display) {
    char title[256];
    snprintf(title, sizeof(title), "tv - %s", display ? display : "");
    SDL_SetWindowTitle(r->window, title);
}

/* Forward decl — overlay helpers live in render.c. */
struct overlay_t;

/* Renders ONE frame right now with the given hint text centered at the
 * bottom. Used by the 'n' handler to flash progress messages between
 * blocking HTTP calls so the user sees each step of the Journaal search
 * unfold live instead of a frozen window. SDL_PumpEvents keeps Windows
 * from marking the window "not responding" during the HTTP round trips. */
static void render_status_frame(render_t *r, overlay_t *ov,
                                SDL_Texture *video_tex, const char *msg) {
    SDL_RenderClear(r->renderer);
    if (video_tex) SDL_RenderCopy(r->renderer, video_tex, NULL, NULL);
    int ww, wh;
    SDL_GetRendererOutputSize(r->renderer, &ww, &wh);
    if (msg && *msg) overlay_render_hint(ov, r->renderer, msg, ww, wh);
    SDL_RenderPresent(r->renderer);
    SDL_PumpEvents();
}

/* Describes a "news programme" that lives across multiple portal catch-up
 * channels — used by the 'n' (NOS Journaal) and 'r' (RTL Nieuws) hotkeys.
 * The search logic scans each archive channel's full EPG for entries whose
 * title starts with title_prefix (case-insensitive) and picks the latest
 * one, preferring airing-now > most-recent-past > soonest-upcoming. */
typedef struct {
    const char          *display_name;   /* shown in toasts: "NOS Journaal" / "RTL Nieuws" */
    const char          *title_prefix;   /* case-insensitive prefix match on EPG titles */
    size_t               prefix_len;     /* strlen(title_prefix) */
    const int           *archive_ids;    /* portal archive-stream IDs (EPG + timeshift) */
    const int           *live_ids;       /* NULL = use NPO live resolution via
                                          * build_channel_url; non-NULL = live IDs
                                          * to use for airing-now / future paths */
    const char * const  *channel_names;  /* human-readable channel names (same length) */
    int                  n_channels;
} news_programme_t;

/* Outcome of find_latest_news: which channel wins + timing flags. */
typedef struct {
    int    channel_idx;    /* -1 if nothing matched */
    int    is_airing;
    int    is_past;
    time_t start;
    time_t end;
} news_hit_t;

static void playback_close(playback_t *pb) {
    if (pb->url) {
        player_stop(&pb->player);
        audio_close(&pb->audio);
        video_tex_destroy(&pb->tex);
        player_close(&pb->player);
        free(pb->url);
    }
    npo_epg_free(&pb->epg);
    memset(pb, 0, sizeof(*pb));
}

static const npo_channel_t *key_to_channel(SDL_Keycode k) {
    switch (k) {
        case SDLK_1: return &NPO_CHANNELS[0];
        case SDLK_2: return &NPO_CHANNELS[1];
        case SDLK_3: return &NPO_CHANNELS[2];
        default: return NULL;
    }
}

/* Resolve the portal archive stream id to use for /timeshift/ replay on the
 * currently-playing channel. The portal serves live and catch-up through
 * distinct IDs: NPO 1/2/3 have the XTREAM_NPO_ARCHIVE_STREAM_IDS mapping,
 * RTL 4/5/7/8/Z have XTREAM_RTL_ARCHIVE_STREAM_IDS. For any other channel
 * we fall back to pb->stream_id — which works for many portals but can
 * return HTTP 502 if the stream lacks tv_archive support (the resulting
 * playback_open failure surfaces as a toast instead of a silent freeze). */
static int resolve_archive_stream_id(const playback_t *pb) {
    if (!pb || !pb->channel) return pb ? pb->stream_id : 0;
    for (int i = 0; i < 3; ++i) {
        if (pb->channel == &NPO_CHANNELS[i] ||
            pb->stream_id == XTREAM_NPO_STREAM_IDS[i] ||
            pb->stream_id == XTREAM_NPO_ARCHIVE_STREAM_IDS[i])
            return XTREAM_NPO_ARCHIVE_STREAM_IDS[i];
    }
    for (int i = 0; i < 5; ++i) {
        if (pb->stream_id == XTREAM_RTL_LIVE_STREAM_IDS[i] ||
            pb->stream_id == XTREAM_RTL_ARCHIVE_STREAM_IDS[i])
            return XTREAM_RTL_ARCHIVE_STREAM_IDS[i];
    }
    return pb->stream_id;   /* best-effort — may 502 if no tv_archive. */
}

/* Human-readable relative day label for an EPG entry's start time. Dutch
 * labels where they fit ("Vandaag"/"Gisteren"/"Morgen"), localised short
 * weekday + date otherwise. Returned pointer is into a static buffer
 * (caller copies out). Keeps the full-EPG list readable when it spans
 * 2-3 days. */
static const char *relative_day_label(time_t t, time_t now, char *buf, size_t buflen) {
    struct tm lt = *localtime(&t);
    struct tm nt = *localtime(&now);
    int lday = lt.tm_year * 400 + lt.tm_yday;
    int nday = nt.tm_year * 400 + nt.tm_yday;
    int diff = lday - nday;
    if (diff == 0)      snprintf(buf, buflen, "Vandaag");
    else if (diff == -1) snprintf(buf, buflen, "Gisteren");
    else if (diff == 1)  snprintf(buf, buflen, "Morgen");
    else {
        static const char *wd[] = { "Zo","Ma","Di","Wo","Do","Vr","Za" };
        snprintf(buf, buflen, "%s %d/%d", wd[lt.tm_wday], lt.tm_mday, lt.tm_mon + 1);
    }
    return buf;
}

/* Parse a language tag from a VOD / series catalog title's trailing
 * parenthesized suffix — "No Time to Die (NL)" → "nld", "Casino Royale
 * (FR)" → "fra", "Jachtseizoen (NL)" → "nld". Xtream catalogs commonly
 * duplicate popular titles with different audio dubs and tag each entry
 * this way; picking the track matching the tag at playback-open is the
 * difference between a user clicking "(NL)" and actually hearing Dutch
 * vs. having to press 'a' to find it.
 *
 * Returns an ISO 639-2 (3-letter) code suitable for matching AVStream
 * metadata["language"], or NULL when no recognisable tag is present.
 * The 2-letter tags are ISO 3166 country codes, mapped here to the
 * corresponding language — pragmatic and matches what these catalogs
 * actually use. Already-3-letter tags pass through lowercased. */
static const char *lang_code_from_title(const char *title) {
    if (!title) return NULL;
    const char *open = NULL;
    for (const char *p = title; *p; ++p) if (*p == '(') open = p;
    if (!open) return NULL;
    const char *close = strchr(open, ')');
    if (!close) return NULL;
    size_t n = (size_t)(close - open - 1);
    if (n < 2 || n > 3) return NULL;
    char tag[4] = {0};
    for (size_t i = 0; i < n; ++i) tag[i] = (char)toupper((unsigned char)open[1+i]);

    if (n == 2) {
        if (!strcmp(tag, "NL")) return "nld";
        if (!strcmp(tag, "FR")) return "fra";
        if (!strcmp(tag, "EN") || !strcmp(tag, "UK") || !strcmp(tag, "US")) return "eng";
        if (!strcmp(tag, "DE")) return "deu";
        if (!strcmp(tag, "ES")) return "spa";
        if (!strcmp(tag, "IT")) return "ita";
        if (!strcmp(tag, "PT") || !strcmp(tag, "BR")) return "por";
        if (!strcmp(tag, "PL")) return "pol";
        if (!strcmp(tag, "TR")) return "tur";
        if (!strcmp(tag, "AR")) return "ara";
        if (!strcmp(tag, "RU")) return "rus";
        return NULL;
    }
    /* 3-letter — assume already ISO 639-2, just lowercase it. */
    static _Thread_local char iso[4];
    for (int i = 0; i < 3; ++i) iso[i] = (char)tolower((unsigned char)tag[i]);
    iso[3] = '\0';
    return iso;
}

/* Try to switch to the audio track matching `want_lang` (ISO 639-2) if one
 * is present. Called after opening a VOD / episode so that "(NL)"-tagged
 * catalog entries immediately play Dutch instead of whichever track the
 * demuxer happened to list first. Silently does nothing when the track
 * isn't found (user can still cycle via 'a'). */
static void apply_preferred_audio_track(player_t *pl, const char *want_lang) {
    if (!pl || !want_lang) return;
    for (int i = 0; i < pl->n_audio_tracks; ++i) {
        if (strcmp(pl->audio_lang[i], want_lang) == 0) {
            if (i != pl->audio_track_cur) {
                fprintf(stderr, "[audio] auto-selecting track %d (%s) to match title\n",
                        i, want_lang);
                player_set_audio_track(pl, i);
            }
            return;
        }
    }
    fprintf(stderr, "[audio] title requests '%s' but no matching track "
                    "(n_tracks=%d) — cycle with 'a'\n",
            want_lang, pl->n_audio_tracks);
}

/* Case-insensitive strstr — portable replacement for GNU's strcasestr, which
 * isn't in MinGW's libc. Used by the search prompt to do a substring match
 * of the user's query against each live-list entry's name. */
static int icase_contains(const char *hay, const char *needle) {
    if (!needle || !*needle) return 1;
    if (!hay) return 0;
    size_t nlen = strlen(needle);
    for (const char *p = hay; *p; ++p) {
        size_t i = 0;
        while (i < nlen && p[i] &&
               tolower((unsigned char)p[i]) == tolower((unsigned char)needle[i]))
            ++i;
        if (i == nlen) return 1;
    }
    return 0;
}

/* Search-hit type: search results merge three distinct catalog kinds so
 * the UI can dispatch differently on Enter. Live hits zap via the async
 * zap pipeline; VOD hits open the /movie/ URL directly; series hits open
 * an episode-picker sub-prompt instead of starting playback. */
typedef enum {
    SEARCH_HIT_LIVE   = 0,
    SEARCH_HIT_VOD    = 1,
    SEARCH_HIT_SERIES = 2,
} search_hit_kind_t;

typedef struct {
    search_hit_kind_t kind;
    int               idx;   /* index into the corresponding catalog list */
} search_hit_t;

/* Refresh hits by matching `query` (case-insensitive substring) across live
 * channels, VOD, and series lists. Search is capped at `cap` total results
 * to keep the overlay readable and to avoid wasting work on e.g. single-
 * letter queries. Match order matches dispatch priority: live first
 * (most common user intent), then series, then VOD — so a query like "the"
 * surfaces channels at the top rather than drowning them in VOD. */
static void search_refresh(const xtream_live_list_t    *live,
                           const xtream_vod_list_t     *vods,
                           const xtream_series_list_t  *series,
                           const char *query, int cap,
                           search_hit_t **hits_out, int *n_out) {
    if (!*query) { *n_out = 0; return; }
    if (!*hits_out) {
        *hits_out = malloc(sizeof(search_hit_t) * (size_t)cap);
        if (!*hits_out) { *n_out = 0; return; }
    }
    int found = 0;
    for (size_t i = 0; live && i < live->count && found < cap; ++i) {
        if (icase_contains(live->entries[i].name, query)) {
            (*hits_out)[found].kind = SEARCH_HIT_LIVE;
            (*hits_out)[found].idx  = (int)i;
            found++;
        }
    }
    for (size_t i = 0; series && i < series->count && found < cap; ++i) {
        if (icase_contains(series->entries[i].name, query)) {
            (*hits_out)[found].kind = SEARCH_HIT_SERIES;
            (*hits_out)[found].idx  = (int)i;
            found++;
        }
    }
    for (size_t i = 0; vods && i < vods->count && found < cap; ++i) {
        if (icase_contains(vods->entries[i].name, query)) {
            (*hits_out)[found].kind = SEARCH_HIT_VOD;
            (*hits_out)[found].idx  = (int)i;
            found++;
        }
    }
    *n_out = found;
}

/* Render-time helper: derives the display label for a search hit.
 * We prefix the catalog-kind so the user can tell at a glance whether
 * Enter will zap, play a VOD, or open an episode picker. */
static const char *search_hit_label(const search_hit_t *h,
                                    const xtream_live_list_t *live,
                                    const xtream_vod_list_t  *vods,
                                    const xtream_series_list_t *series,
                                    char *buf, size_t buflen) {
    const char *name = "";
    const char *tag  = "";
    switch (h->kind) {
    case SEARCH_HIT_LIVE:   tag = "[LIVE]  "; name = live->entries[h->idx].name;   break;
    case SEARCH_HIT_VOD:    tag = "[MOVIE] "; name = vods->entries[h->idx].name;   break;
    case SEARCH_HIT_SERIES: tag = "[SERIES] "; name = series->entries[h->idx].name; break;
    }
    snprintf(buf, buflen, "%s%s", tag, name);
    return buf;
}

typedef struct {
    int    channel_idx;   /* 0..2, or -1 if nothing found */
    int    is_airing;     /* 1 if currently on air, 0 if already ended or future */
    int    is_past;       /* 1 if the winning entry already ended */
    time_t start;         /* start time (unix) of the winning entry */
    time_t end;           /* end time (unix) of the winning entry */
    long long seconds_ago;/* how long ago it started (positive = past) */
    long long seconds_til;/* how long until start (positive = future) */
} journaal_hit_t;

/* Find the LATEST NOS Journaal across NPO 1/2/3 — i.e. the most-recently-started
 * one. Preference:
 *   1. currently airing  (start <= now < end) — pick the latest such entry
 *   2. most recent past  (end <= now)         — pick the latest ended one
 *   3. soonest upcoming  (start > now)        — only as last resort
 *
 * Fetches EPG for each NPO channel via the portal (~3 HTTP calls, ~1s total). */
static journaal_hit_t find_latest_journaal(const xtream_t *portal) {
    journaal_hit_t hit = { .channel_idx = -1 };
    if (!portal || !portal->host) return hit;
    time_t now = time(NULL);

    /* Track three candidates separately so we can pick in preference order. */
    int    best_airing_ch = -1; time_t best_airing_start = 0; time_t best_airing_end = 0;
    int    best_past_ch   = -1; time_t best_past_start   = 0; time_t best_past_end   = 0;
    int    best_next_ch   = -1; time_t best_next_start   = 0; time_t best_next_end   = 0;

    /* Use the FULL EPG (get_simple_data_table) on the archive IDs. The
     * live IDs' get_short_epg only returns ~4 upcoming entries and never
     * contains past journaals — that's why the previous version always
     * reported "NOS Journaal not found in EPG". The archive stream has
     * the same programme guide for its channel but the portal exposes
     * 800+ entries spanning the full 2-day catch-up window. */
    for (int i = 0; i < 3; ++i) {
        epg_t e = {0};
        if (xtream_fetch_epg_full(portal, XTREAM_NPO_ARCHIVE_STREAM_IDS[i], &e) != 0) {
            fprintf(stderr, "[journaal] EPG fetch failed for archive id %d\n",
                    XTREAM_NPO_ARCHIVE_STREAM_IDS[i]);
            continue;
        }
        int journaals_seen = 0;
        for (size_t j = 0; j < e.count; ++j) {
            if (!e.entries[j].is_news) continue;
            journaals_seen++;
            time_t s = e.entries[j].start, en = e.entries[j].end;
            if (s <= now && now < en) {
                if (s > best_airing_start) {
                    best_airing_ch = i; best_airing_start = s; best_airing_end = en;
                }
            } else if (en <= now) {
                if (s > best_past_start) {
                    best_past_ch = i; best_past_start = s; best_past_end = en;
                }
            } else { /* s > now */
                if (best_next_ch < 0 || s < best_next_start) {
                    best_next_ch = i; best_next_start = s; best_next_end = en;
                }
            }
        }
        fprintf(stderr, "[journaal] scanned NPO %d archive: %zu entries, %d journaals\n",
                i + 1, e.count, journaals_seen);
        npo_epg_free(&e);
    }

    if (best_airing_ch >= 0) {
        hit.channel_idx = best_airing_ch;
        hit.is_airing   = 1;
        hit.start       = best_airing_start;
        hit.end         = best_airing_end;
        hit.seconds_ago = (long long)(now - best_airing_start);
    } else if (best_past_ch >= 0) {
        hit.channel_idx = best_past_ch;
        hit.is_past     = 1;
        hit.start       = best_past_start;
        hit.end         = best_past_end;
        hit.seconds_ago = (long long)(now - best_past_end);
    } else if (best_next_ch >= 0) {
        hit.channel_idx = best_next_ch;
        hit.start       = best_next_start;
        hit.end         = best_next_end;
        hit.seconds_til = (long long)(best_next_start - now);
    }
    return hit;
}

/* --- Background zap preparation, 3-phase, per-request allocation ---
 *
 * UX goal: channel name shows INSTANTLY on wheel commit, then programme
 * title is filled in as soon as the EPG comes back (~200-400ms), then the
 * stream swaps when libav finishes probing (~1-2s). Main never blocks.
 *
 * Critical design choice: each zap request gets its OWN heap-allocated
 * zap_prep_t. When the user cancels (fast wheel scrolling), we set
 * state=5 on the old prep, detach its thread, and null out our pointer —
 * the detached worker sees state=5, frees everything it holds (including
 * the struct itself because self_owned=1), and vanishes. Main immediately
 * allocates a fresh struct for the new request. Two workers running
 * simultaneously is fine because they touch disjoint structs.
 *
 * This fixes a fast-scroll crash where the old code reused a single
 * shared struct and the new request would stomp the old worker's state
 * mid-flight.
 *
 * State machine:
 *   0 running, EPG fetch in progress     (initial after pthread_create)
 *   2 EPG ready, stream open in progress (worker -> main signal)
 *   3 stream ready, pb valid             (worker -> main signal)
 *   4 failed (worker owns cleanup)
 *   5 cancelled by main (worker owns cleanup, frees struct too)
 *
 * Ownership:
 *   - main allocates zp, fills inputs, spawns worker, assigns active_prep.
 *   - Worker frees zp->url and owns lifetime of preload_epg + pb until
 *     state >= 2 or 3 (handoff) OR state == 4/5 (worker deallocates).
 *   - If self_owned, worker frees zp itself at exit. */
typedef struct {
    pthread_t             thread;
    volatile int          state;          /* see comment above */
    volatile int          self_owned;     /* 1 = worker frees zp on exit */
    xtream_t             *portal;
    render_t             *render;
    char                 *url;
    int                   epg_stream_id;
    const npo_channel_t  *channel;
    char                  label[192];
    int                   list_idx;

    epg_t                 preload_epg;
    int                   epg_toast_shown; /* main: "I already baked this EPG into the toast" */
    playback_t           *pb;
    int                   probe_failed;    /* 1 if health probe rejected the URL */
} zap_prep_t;

static void *zap_prep_worker(void *arg) {
    zap_prep_t *zp = arg;

    /* Phase 1: EPG fetch. Usually 200-400ms. */
    epg_t ep = {0};
    (void)xtream_fetch_epg(zp->portal, zp->epg_stream_id, &ep);
    if (zp->state == 5) {
        npo_epg_free(&ep);
        free(zp->url); zp->url = NULL;
        if (zp->self_owned) free(zp);
        return NULL;
    }
    zp->preload_epg = ep;
    zp->state = 2;

    /* Phase 1.5: liveness probe. A HEAD request on the stream URL tells us
     * in 1-2s whether the channel is actually serving right now. Without
     * this, a dead channel lets playback_open block for 15-30s while libav
     * reconnects + retries, then finally returns an error — terrible UX.
     * 3-second timeout is aggressive enough to keep fast zapping snappy
     * while tolerating a slow-but-working CDN hop. */
    if (npo_http_probe(zp->url, 3) != 0) {
        zp->probe_failed = 1;
        goto fail;
    }

    /* Phase 2: open playback. epg_stream_id=-1 skips the internal EPG
     * fetch because we already have the EPG ready to splice in. */
    playback_t *pb = calloc(1, sizeof(*pb));
    if (!pb) goto fail;
    int rc = playback_open(pb, zp->render, zp->channel, zp->url, zp->portal, -1);
    free(zp->url); zp->url = NULL;
    if (rc != 0) { free(pb); goto fail; }

    /* Splice the pre-fetched EPG in — move, don't copy. */
    npo_epg_free(&pb->epg);
    pb->epg = zp->preload_epg;
    memset(&zp->preload_epg, 0, sizeof(zp->preload_epg));

    if (zp->state == 5) {
        playback_close(pb); free(pb);
        if (zp->self_owned) free(zp);
        return NULL;
    }
    zp->pb    = pb;
    zp->state = 3;
    /* Main will consume and free zp on state==3; we don't. */
    return NULL;

fail:
    npo_epg_free(&zp->preload_epg);
    zp->state = 4;
    if (zp->self_owned) free(zp);
    return NULL;
}

int main(int argc, char **argv) {
    /* Line-buffer stdout so diagnostic prints appear in real time even when
     * redirected to a file (default fully-buffered would only flush at exit,
     * hiding what happened right before a crash). */
    setvbuf(stdout, NULL, _IOLBF, 0);

    /* Silence libav's info-level chatter ("Opening '...ts'" on every segment).
     * We keep warnings/errors, which is where real diagnostics live. Our own
     * [decoder]/[stall] fprintf(stderr, ...) lines are untouched. */
    av_log_set_level(AV_LOG_WARNING);

    /* Test-harness env vars for the seek-bug ralph loop:
     *   TV_AUTOSEEK_AT_S       — seconds after launch to inject a right-arrow
     *                            keypress (simulates user pressing skip +30s).
     *   TV_TEST_EXIT_AFTER_S   — seconds after launch to auto-quit, so the
     *                            harness can cap test duration.
     *   TV_TEST_HEARTBEAT_MS   — override the normal 15s heartbeat cadence
     *                            for finer-grained logs during the test.
     * When any of these are set, we also dump decoder state on every frame
     * for the 20s window around the seek. */
    double autoseek_at   = getenv("TV_AUTOSEEK_AT_S")     ? atof(getenv("TV_AUTOSEEK_AT_S"))     : 0.0;
    double test_exit_at  = getenv("TV_TEST_EXIT_AFTER_S") ? atof(getenv("TV_TEST_EXIT_AFTER_S")) : 0.0;
    int    test_hb_ms    = getenv("TV_TEST_HEARTBEAT_MS") ? atoi(getenv("TV_TEST_HEARTBEAT_MS")) : 0;
    int    autozap_count = getenv("TV_AUTOZAP_DOWN")      ? atoi(getenv("TV_AUTOZAP_DOWN"))      : 0;
    double autozap_start = getenv("TV_AUTOZAP_AT_S")      ? atof(getenv("TV_AUTOZAP_AT_S"))      : 5.0;
    double autozap_spacing_s = 1.5;  /* > WHEEL_DEBOUNCE_MS so each press is a distinct zap */
    int    autozap_fired = 0;
    Uint32 test_launch_ms = SDL_GetTicks();
    int    autoseek_fired = 0;
    if (autoseek_at > 0 || test_exit_at > 0 || autozap_count > 0)
        fprintf(stderr, "[test] autoseek=%.1fs autozap=%dx(at %.1fs) exit=%.1fs hb=%dms\n",
                autoseek_at, autozap_count, autozap_start, test_exit_at, test_hb_ms);

    curl_global_init(CURL_GLOBAL_DEFAULT);

    /* Parse CLI: supports `--xtream user:pass@host[:port]` OR a bare URL as argv[1]. */
    xtream_t portal = {0};
    const char *direct_url = NULL;
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--xtream") == 0 && i + 1 < argc) {
            if (xtream_parse(argv[++i], &portal) != 0) {
                fprintf(stderr, "bad --xtream spec (expected user:pass@host[:port])\n");
                return 2;
            }
        } else if (!direct_url) {
            direct_url = argv[i];
        }
    }
    if (portal.host) {
        printf("xtream portal: %s:%d user=%s\n", portal.host, portal.port, portal.user);
    } else if (direct_url) {
        printf("direct URL: %s\n", direct_url);
    }

    render_t r;
    if (render_init(&r, 960, 540, "tv - booting") != 0) return 1;

    /* IMPORTANT: playback_t is HEAP-allocated, not stack. The audio callback
     * userdata and av_clock point INTO this struct (at &pb->audio, etc.); if
     * we ever assign `pb = new_pb` by value, those pointers would dangle to
     * a scope-local new_pb. Heap + pointer-swap avoids the UAF. */
    char *initial_url = build_channel_url(0, &portal, direct_url);
    playback_t *pb = calloc(1, sizeof(*pb));
    if (!pb || playback_open(pb, &r, &NPO_CHANNELS[0], initial_url, &portal, 0) != 0) {
        fputs("initial playback_open failed\n"
              "  try: ./tv.exe --xtream user:pass@host:port\n"
              "  or:  ./tv.exe https://some/stream.m3u8\n", stderr);
        free(pb);
        free(initial_url);
        xtream_free(&portal);
        render_shutdown(&r);
        return 2;
    }
    set_window_title(&r, NPO_CHANNELS[0].display);
    free(initial_url);

    overlay_t ov;
    if (overlay_init(&ov, "assets/DejaVuSans.ttf") != 0) {
        playback_close(pb);
        free(pb);
        render_shutdown(&r);
        return 5;
    }
    int show_overlay   = 1;
    int running        = 1;
    int paused         = 0;
    int have_texture   = 0;
    video_frame_t *pending_vf = NULL;  /* held across iterations when not yet due */
    /* Audio-callback liveness tracking. samples_played advances as long as
     * SDL's audio callback is being called — it's incremented even when the
     * callback writes silence (no chunks). So if it stops advancing, the
     * callback has stopped being called, which is the Windows/SDL audio
     * subsystem glitch we want to recover from. */
    int64_t prev_samples_played      = 0;
    Uint32  last_audio_progress_ts   = SDL_GetTicks();

    /* Timestamp of the most recent successful playback_open (initial boot,
     * stall-restart, zap swap, channel switch, etc.). Used by the stall
     * detector to apply a grace period before declaring "audio never started"
     * on a fresh playback — it takes a few seconds for libav to probe and
     * the decoder thread to push the first audio chunk. */
    Uint32  playback_open_ts         = SDL_GetTicks();

    /* Heartbeat: log state every HEARTBEAT_MS so we can see in the log
     * exactly when main stops making progress. If the heartbeat pauses,
     * main thread is blocked somewhere. */
    Uint32 last_heartbeat = SDL_GetTicks();
    const Uint32 HEARTBEAT_MS = 15000;
    /* If no audio progress (has_first_pts stays 0) after STARTUP_MS on a fresh
     * playback, something went wrong during open (decode stalled, network
     * died mid-probe). Restart to recover. 10s is comfortably above the
     * normal first-frame latency (2-4s observed) but well below the user's
     * "did it crash?" threshold. */
    const Uint32 STARTUP_MS   = 10000;

    /* Audio warmup. SDL audio device starts paused (see audio_open). Main
     * unpauses when the decoder has pushed at least one video frame OR
     * AUDIO_WARMUP_MAX_MS have elapsed since playback_open (failsafe for
     * audio-only streams / broken video decoders). Reset on every pb swap. */
    int    audio_warmed              = 0;
    const Uint32 AUDIO_WARMUP_MAX_MS = 3000;

    /* Zap state: direction of the last committed wheel delta (±1), used by
     * the state==4 path to auto-advance past dead channels in the same
     * direction instead of parking the user on "X unavailable". zap_skip
     * bounds the auto-advance chain so a run of 50 dead channels doesn't
     * zap forever; reset on a successful state==3 swap. */
    int    zap_direction             = 0;
    int    zap_skip_count            = 0;
    const int ZAP_SKIP_LIMIT         = 20;

    /* Right-click drag state — since the window is borderless there's no title
     * bar to grab, so we implement window-move by: on right-button-down we
     * snapshot the global mouse and window positions; on right-button-up we
     * stop; every iteration while dragging we move the window by the current
     * global mouse delta relative to the snapshot. SDL_CaptureMouse lets the
     * drag continue even when the cursor leaves the window bounds. */
    int drag_active = 0;
    int drag_anchor_mx = 0, drag_anchor_my = 0;
    int drag_anchor_wx = 0, drag_anchor_wy = 0;

    /* Left double-click cycles the window through a set of zoom factors.
     * Base size is the window's initial dimensions (960x540). */
    static const float SIZE_FACTORS[] = { 1.0f, 1.5f, 2.0f, 3.0f, 4.0f };
    const int  SIZE_COUNT = (int)(sizeof(SIZE_FACTORS) / sizeof(SIZE_FACTORS[0]));
    int        size_idx   = 0;
    const int  base_w     = 960;
    const int  base_h     = 540;

    /* Help overlay state.
     *  help_visible   — toggled by '?' key
     *  hint_until_ms  — while SDL_GetTicks() < this, show a transient hint
     *                   at bottom-right suggesting "Press ? for help" */
    int    help_visible  = 0;
    Uint32 hint_until_ms = SDL_GetTicks() + 8000;  /* 8 seconds after start */

    /* Toast state — short message shown in the bottom-right corner for a few
     * seconds. Written by actions like 'n' (Journaal search) so the user gets
     * visible feedback regardless of stderr visibility. */
    char    toast_text[512] = {0};  /* room for channel + " | " + programme title */
    Uint32  toast_until_ms  = 0;

    /* Search prompt ('f' key). search_active toggles text-input mode; every
     * keystroke refreshes search_hits by substring-matching across live
     * channels + VOD + series. Enter dispatches per hit kind:
     *   LIVE   -> zap via pending_wheel_delta
     *   VOD    -> fetch /movie URL + playback_open directly
     *   SERIES -> drop into EPISODE_PICKER with the series's episode list */
    int            search_active    = 0;
    char           search_query[128] = {0};
    int            search_query_len  = 0;
    search_hit_t  *search_hits       = NULL;
    int            search_hits_count = 0;
    int            search_sel        = 0;
    /* SDL fires SDL_TEXTINPUT for the same 'f' keypress that opens the
     * search prompt (we enable text input mid-dispatch, and the OS has
     * already queued the 'f' character). Without this flag the search box
     * opens with "f" already typed. Set on 'f'-down, honoured-and-cleared
     * by the next TEXTINPUT event. */
    int            search_swallow_next_text = 0;
    const int SEARCH_VISIBLE = 12;     /* rows shown in the result list */

    /* Episode-picker sub-mode: opened by Enter on a SERIES hit. We fetch
     * get_series_info synchronously (only ~200-500 ms for a typical series),
     * show the flat episode list, and handle it with the same nav keys as
     * the main search. Esc returns to the search results the user was on. */
    int                  episode_picker_active = 0;
    xtream_episodes_t    episodes              = {0};
    int                  episode_sel           = 0;
    char                 episode_series_name[256] = {0};

    /* Full-EPG list overlay: Shift+e opens a scrollable multi-day guide for
     * the current channel (via xtream_fetch_epg_full ~ 800 entries over the
     * 2-day catch-up window). Enter on a past entry plays /timeshift at that
     * start time; Enter on the airing-now entry is a no-op; Enter on a
     * future entry shows a "not aired yet" toast. */
    int       epg_full_active     = 0;
    epg_t     epg_full            = {0};
    int       epg_full_sel        = 0;
    int       epg_full_archive_id = 0;
    char      epg_full_channel_name[128] = {0};

    /* Mouse-wheel zapping through ALL portal live channels. We fetch the full
     * catalog once, find our current stream_id in it, and the wheel moves the
     * index ±1 per notch. Actual switch is debounced — rapid scrolling only
     * triggers one playback_open after the wheel stops for WHEEL_DEBOUNCE_MS,
     * which matters because the portal caps concurrent connections at 1. */
    xtream_live_list_t    live_list   = {0};
    xtream_vod_list_t     vod_list    = {0};
    xtream_series_list_t  series_list = {0};
    int    current_live_idx = -1;
    int    pending_wheel_delta = 0;
    Uint32 last_wheel_ts = 0;
    const Uint32 WHEEL_DEBOUNCE_MS = 350;

    /* Background zap prep: see zap_prep_t docs above. Pointer, not inline —
     * each wheel commit that cancels a prior prep leaves the old detached
     * worker to clean its OWN struct, and main immediately allocates a
     * fresh one for the next prep. Two workers can coexist safely because
     * their structs are disjoint. */
    zap_prep_t *zap_prep = NULL;

    /* Diagnostic: if TV_TEST_JOURNAAL env var is set, call the Journaal
     * search once at startup and log the result. Lets automated smoke
     * tests verify the lookup works without synthesizing key events. */
    if (portal.host && getenv("TV_TEST_JOURNAAL")) {
        journaal_hit_t h = find_latest_journaal(&portal);
        fprintf(stderr, "[test-journaal] ch=%d airing=%d past=%d future=%d start=%ld end=%ld\n",
                h.channel_idx, h.is_airing, h.is_past, (h.channel_idx >= 0 && !h.is_airing && !h.is_past) ? 1 : 0,
                (long)h.start, (long)h.end);
    }

    if (portal.host) {
        if (xtream_fetch_live_list(&portal, NULL, &live_list) == 0) {
            fprintf(stderr, "[zap] loaded %zu channels from portal\n", live_list.count);
            for (size_t i = 0; i < live_list.count; ++i) {
                if (live_list.entries[i].stream_id == XTREAM_NPO_STREAM_IDS[0]) {
                    current_live_idx = (int)i;
                    break;
                }
            }
        } else {
            fprintf(stderr, "[zap] failed to fetch live channel list — wheel disabled\n");
        }
        /* VOD + series are used only by the 'f' search prompt. A fetch
         * failure (portal 403 on un-subscribed catalogs, brief 5xx) is
         * non-fatal — the lists stay empty and search just returns fewer
         * results. Don't gate live-TV startup on these. */
        if (xtream_fetch_vod_list(&portal, NULL, &vod_list) == 0)
            fprintf(stderr, "[search] loaded %zu VOD entries from portal\n", vod_list.count);
        else
            fprintf(stderr, "[search] VOD catalog unavailable — search will skip VOD\n");
        if (xtream_fetch_series_list(&portal, NULL, &series_list) == 0)
            fprintf(stderr, "[search] loaded %zu series from portal\n", series_list.count);
        else
            fprintf(stderr, "[search] series catalog unavailable — search will skip series\n");
    }

    /* Sync tolerances (seconds):
     *  DUE_WINDOW  — how "early" a frame can be to count as due-now (one vsync @ 60Hz ≈ 16ms)
     *  DROP_LATE   — how late a frame can be before we drop it (80ms ≈ 2 frames at 25fps) */
    const double DUE_WINDOW    = 0.010;
    const double DROP_LATE     = 0.080;
    /* Stall detection: if we've had a texture up and no new frame arrives for
     * STALL_MS, assume the decoder thread died (network drop, HLS sequence
     * reset, portal timeout) and restart the same channel. */
    const Uint32 STALL_MS      = 3000;

    while (running) {
        /* 1) Events first, every iteration — keeps UI responsive regardless of decode state. */
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_QUIT) { running = 0; break; }
            if (ev.type == SDL_MOUSEBUTTONDOWN && ev.button.button == SDL_BUTTON_RIGHT) {
                SDL_GetGlobalMouseState(&drag_anchor_mx, &drag_anchor_my);
                SDL_GetWindowPosition(r.window, &drag_anchor_wx, &drag_anchor_wy);
                drag_active = 1;
                SDL_CaptureMouse(SDL_TRUE);
                continue;
            }
            if (ev.type == SDL_MOUSEBUTTONDOWN && ev.button.button == SDL_BUTTON_LEFT &&
                ev.button.clicks == 2 && !r.fullscreen) {
                size_idx = (size_idx + 1) % SIZE_COUNT;
                int new_w = (int)(base_w * SIZE_FACTORS[size_idx]);
                int new_h = (int)(base_h * SIZE_FACTORS[size_idx]);
                SDL_SetWindowSize(r.window, new_w, new_h);
                SDL_SetWindowPosition(r.window, SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED);
                fprintf(stderr, "[size] %.1fx (%dx%d)\n", SIZE_FACTORS[size_idx], new_w, new_h);
                continue;
            }
            if (ev.type == SDL_MOUSEBUTTONUP && ev.button.button == SDL_BUTTON_RIGHT) {
                drag_active = 0;
                SDL_CaptureMouse(SDL_FALSE);
                continue;
            }
            if (ev.type == SDL_MOUSEWHEEL && current_live_idx >= 0 && !search_active) {
                /* wheel up (positive y) = previous channel; down = next channel */
                pending_wheel_delta -= ev.wheel.y;
                last_wheel_ts = SDL_GetTicks();
                continue;
            }

            /* Full-EPG overlay (Shift+e). Catches keys before search/episode
             * modes so Enter / arrows route to EPG navigation. */
            if (epg_full_active) {
                if (ev.type == SDL_KEYDOWN) {
                    SDL_Keycode sk = ev.key.keysym.sym;
                    int n = (int)epg_full.count;
                    if (sk == SDLK_ESCAPE) {
                        epg_full_active = 0;
                    } else if (sk == SDLK_UP && n > 0) {
                        epg_full_sel = (epg_full_sel - 1 + n) % n;
                    } else if (sk == SDLK_DOWN && n > 0) {
                        epg_full_sel = (epg_full_sel + 1) % n;
                    } else if ((sk == SDLK_PAGEUP || sk == SDLK_PAGEDOWN) && n > 0) {
                        int delta = sk == SDLK_PAGEUP ? -10 : +10;
                        epg_full_sel = epg_full_sel + delta;
                        if (epg_full_sel < 0) epg_full_sel = 0;
                        if (epg_full_sel >= n) epg_full_sel = n - 1;
                    } else if ((sk == SDLK_RETURN || sk == SDLK_KP_ENTER) &&
                               epg_full_sel < n) {
                        epg_entry_t *e = &epg_full.entries[epg_full_sel];
                        time_t tnow = time(NULL);
                        if (e->start > tnow) {
                            snprintf(toast_text, sizeof(toast_text),
                                     "Programme hasn't started yet");
                            toast_until_ms = SDL_GetTicks() + 3000;
                        } else {
                            /* Past or currently-airing — open via timeshift so
                             * a long-running live programme also rewinds to its
                             * own start. Duration rounded up to nearest min. */
                            int dur_min = (int)((e->end - e->start + 59) / 60);
                            if (dur_min < 5) dur_min = 5;
                            char *url = xtream_timeshift_url(&portal,
                                                             epg_full_archive_id,
                                                             e->start, dur_min);
                            snprintf(toast_text, sizeof(toast_text),
                                     "Playing %s", e->title ? e->title : "(programme)");
                            toast_until_ms = SDL_GetTicks() + 5000;

                            playback_t *new_pb = calloc(1, sizeof(*new_pb));
                            if (url && new_pb && playback_open(new_pb, &r,
                                                               pb->channel, url,
                                                               &portal,
                                                               epg_full_archive_id) == 0) {
                                new_pb->timeshift_start   = e->start;
                                new_pb->timeshift_dur_min = dur_min;
                                playback_close(pb); free(pb); pb = new_pb;
                                paused = 0; have_texture = 0;
                                if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                                overlay_mark_dirty(&ov);
                                playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
                                last_audio_progress_ts = SDL_GetTicks();
                                prev_samples_played    = 0;
                            } else {
                                free(new_pb);
                                snprintf(toast_text, sizeof(toast_text),
                                         "Couldn't open catch-up stream");
                            }
                            free(url);
                            epg_full_active = 0;
                        }
                    }
                }
                continue;
            }

            /* Episode picker: active after pressing Enter on a SERIES hit.
             * Takes precedence over search so we don't get confusing key
             * dispatch while the user is drilling into a series. */
            if (episode_picker_active) {
                if (ev.type == SDL_KEYDOWN) {
                    SDL_Keycode sk = ev.key.keysym.sym;
                    if (sk == SDLK_ESCAPE) {
                        /* Back to search results. */
                        episode_picker_active = 0;
                        xtream_episodes_free(&episodes);
                        search_active = 1;
                        SDL_StartTextInput();
                    } else if (sk == SDLK_UP) {
                        if (episodes.count > 0)
                            episode_sel = (int)((episode_sel - 1 + (int)episodes.count)
                                                % (int)episodes.count);
                    } else if (sk == SDLK_DOWN) {
                        if (episodes.count > 0)
                            episode_sel = (int)((episode_sel + 1) % (int)episodes.count);
                    } else if (sk == SDLK_RETURN || sk == SDLK_KP_ENTER) {
                        if (episodes.count > 0 && episode_sel < (int)episodes.count) {
                            xtream_episode_entry_t *ep = &episodes.entries[episode_sel];
                            char *url = xtream_series_episode_url(&portal, ep->id, ep->extension);
                            snprintf(toast_text, sizeof(toast_text),
                                     "Opening %s S%dE%d %s",
                                     episode_series_name, ep->season_num, ep->episode_num,
                                     ep->title ? ep->title : "");
                            toast_until_ms = SDL_GetTicks() + 5000;

                            playback_t *new_pb = calloc(1, sizeof(*new_pb));
                            if (url && new_pb && playback_open(new_pb, &r,
                                                               pb->channel, url,
                                                               &portal, -1) == 0) {
                                playback_close(pb); free(pb); pb = new_pb;
                                paused = 0; have_texture = 0;
                                if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                                overlay_mark_dirty(&ov);
                                playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
                                last_audio_progress_ts = SDL_GetTicks();
                                prev_samples_played    = 0;
                                current_live_idx = -1;   /* not a live channel any more */
                                /* Series titles sometimes carry the same
                                 * "(NL)"/"(FR)" language tags as VOD (e.g.
                                 * "Jachtseizoen (NL)"). Apply the same
                                 * preference so episodes play the right dub. */
                                apply_preferred_audio_track(&pb->player,
                                                            lang_code_from_title(episode_series_name));
                            } else {
                                free(new_pb);
                                snprintf(toast_text, sizeof(toast_text),
                                         "Failed to open episode");
                            }
                            free(url);
                        }
                        episode_picker_active = 0;
                        xtream_episodes_free(&episodes);
                    }
                }
                continue;
            }

            /* Search mode intercepts text input and most keys so typed
             * characters don't quit/fullscreen/toggle things by accident. */
            if (search_active) {
                if (ev.type == SDL_TEXTINPUT) {
                    if (search_swallow_next_text) {
                        /* Drop the TEXTINPUT that SDL queued for the 'f'
                         * key which opened this prompt. */
                        search_swallow_next_text = 0;
                        continue;
                    }
                    size_t add = strlen(ev.text.text);
                    if (search_query_len + add < sizeof(search_query) - 1) {
                        memcpy(search_query + search_query_len, ev.text.text, add);
                        search_query_len += (int)add;
                        search_query[search_query_len] = '\0';
                        search_refresh(&live_list, &vod_list, &series_list,
                                       search_query, SEARCH_VISIBLE,
                                       &search_hits, &search_hits_count);
                        search_sel = 0;
                    }
                    continue;
                }
                if (ev.type == SDL_KEYDOWN) {
                    SDL_Keycode sk = ev.key.keysym.sym;
                    if (sk == SDLK_ESCAPE) {
                        search_active = 0;
                        SDL_StopTextInput();
                    } else if (sk == SDLK_BACKSPACE) {
                        if (search_query_len > 0) {
                            /* UTF-8 safe: drop trailing continuation bytes
                             * until we hit the lead byte. */
                            --search_query_len;
                            while (search_query_len > 0 &&
                                   (search_query[search_query_len] & 0xC0) == 0x80)
                                --search_query_len;
                            search_query[search_query_len] = '\0';
                            search_refresh(&live_list, &vod_list, &series_list,
                                           search_query, SEARCH_VISIBLE,
                                           &search_hits, &search_hits_count);
                            search_sel = 0;
                        }
                    } else if (sk == SDLK_UP) {
                        if (search_hits_count > 0)
                            search_sel = (search_sel - 1 + search_hits_count) % search_hits_count;
                    } else if (sk == SDLK_DOWN) {
                        if (search_hits_count > 0)
                            search_sel = (search_sel + 1) % search_hits_count;
                    } else if (sk == SDLK_RETURN || sk == SDLK_KP_ENTER) {
                        if (search_hits_count > 0 && search_sel < search_hits_count) {
                            search_hit_t h = search_hits[search_sel];
                            if (h.kind == SEARCH_HIT_LIVE) {
                                /* Route through the same async zap pipeline
                                 * the wheel uses. Back-date last_wheel_ts so
                                 * the debounce is already elapsed and the
                                 * zap fires on the very next iteration. */
                                pending_wheel_delta = h.idx - current_live_idx;
                                last_wheel_ts = SDL_GetTicks() - 400;
                                search_active = 0;
                                SDL_StopTextInput();
                            } else if (h.kind == SEARCH_HIT_VOD) {
                                xtream_vod_entry_t *v = &vod_list.entries[h.idx];
                                char *url = xtream_vod_url(&portal, v->stream_id, v->extension);
                                snprintf(toast_text, sizeof(toast_text), "Opening %s", v->name);
                                toast_until_ms = SDL_GetTicks() + 5000;
                                playback_t *new_pb = calloc(1, sizeof(*new_pb));
                                if (url && new_pb && playback_open(new_pb, &r,
                                                                   pb->channel, url,
                                                                   &portal, -1) == 0) {
                                    playback_close(pb); free(pb); pb = new_pb;
                                    paused = 0; have_texture = 0;
                                    if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                                    overlay_mark_dirty(&ov);
                                    playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
                                    last_audio_progress_ts = SDL_GetTicks();
                                    prev_samples_played    = 0;
                                    current_live_idx = -1;
                                    /* Honour the "(NL)"/"(FR)" suffix in the
                                     * catalog entry — user picked that one
                                     * because they want that language. */
                                    apply_preferred_audio_track(&pb->player,
                                                                lang_code_from_title(v->name));
                                    /* Surface the actual playing track so
                                     * the user can spot mislabeled catalog
                                     * entries immediately (file says "(NL)"
                                     * but only has French audio — they'll
                                     * see "audio: fra" and know to pick a
                                     * different one). */
                                    const char *cur_lang =
                                        pb->player.n_audio_tracks > 0
                                        ? pb->player.audio_lang[pb->player.audio_track_cur]
                                        : "";
                                    snprintf(toast_text, sizeof(toast_text),
                                             "Playing %s  —  audio: %s%s",
                                             v->name,
                                             cur_lang[0] ? cur_lang : "unknown",
                                             pb->player.n_audio_tracks > 1
                                             ? " (press 'a' to switch)" : "");
                                    toast_until_ms = SDL_GetTicks() + 6000;
                                } else {
                                    free(new_pb);
                                    snprintf(toast_text, sizeof(toast_text),
                                             "Failed to open %s", v->name);
                                }
                                free(url);
                                search_active = 0;
                                SDL_StopTextInput();
                            } else if (h.kind == SEARCH_HIT_SERIES) {
                                xtream_series_entry_t *s = &series_list.entries[h.idx];
                                /* Synchronous fetch — typically 200-500ms.
                                 * Quick toast first so the frame after Enter
                                 * shows "Loading…" instead of a frozen UI. */
                                snprintf(toast_text, sizeof(toast_text),
                                         "Loading %s episodes…", s->name);
                                render_status_frame(&r, &ov, pb->tex.texture, toast_text);
                                xtream_episodes_free(&episodes);
                                if (xtream_fetch_series_info(&portal, s->series_id,
                                                              &episodes) == 0 &&
                                    episodes.count > 0) {
                                    snprintf(episode_series_name,
                                             sizeof(episode_series_name), "%s", s->name);
                                    episode_sel = 0;
                                    episode_picker_active = 1;
                                    search_active = 0;
                                    SDL_StopTextInput();
                                } else {
                                    snprintf(toast_text, sizeof(toast_text),
                                             "No episodes found for %s", s->name);
                                    toast_until_ms = SDL_GetTicks() + 4000;
                                    search_active = 0;
                                    SDL_StopTextInput();
                                }
                            }
                        } else {
                            search_active = 0;
                            SDL_StopTextInput();
                        }
                    }
                }
                continue;
            }

            if (ev.type != SDL_KEYDOWN) continue;
            SDL_Keycode k = ev.key.keysym.sym;
            if (k == SDLK_q || k == SDLK_ESCAPE) { running = 0; break; }
            else if (k == SDLK_f) {
                /* Remapped in 2026-04-18: 'f' opens the search prompt (was
                 * fullscreen). Fullscreen moved to F11. */
                search_active = 1;
                search_query[0] = '\0';
                search_query_len = 0;
                search_hits_count = 0;
                search_sel = 0;
                search_swallow_next_text = 1;
                SDL_StartTextInput();
            }
            else if (k == SDLK_F11) render_toggle_fullscreen(&r);
            else if (k == SDLK_SPACE) {
                paused = !paused;
                SDL_PauseAudioDevice(pb->audio.device, paused);
            }
            else if (k == SDLK_e) {
                /* Shift+e opens the full multi-day EPG list (catch-up + future).
                 * Plain 'e' toggles the compact bottom-strip overlay. */
                if (ev.key.keysym.mod & KMOD_SHIFT) {
                    int archive_id = resolve_archive_stream_id(pb);
                    if (archive_id <= 0 || !portal.host) {
                        snprintf(toast_text, sizeof(toast_text),
                                 "Full EPG not available on this channel");
                        toast_until_ms = SDL_GetTicks() + 3000;
                    } else {
                        snprintf(toast_text, sizeof(toast_text),
                                 "Loading full EPG…");
                        render_status_frame(&r, &ov, pb->tex.texture, toast_text);
                        npo_epg_free(&epg_full);
                        if (xtream_fetch_epg_full(&portal, archive_id, &epg_full) == 0
                            && epg_full.count > 0) {
                            /* Start the selection on the currently-airing
                             * entry if we can find one, else at index 0. */
                            time_t tnow = time(NULL);
                            epg_full_sel = 0;
                            for (size_t i = 0; i < epg_full.count; ++i) {
                                if (epg_full.entries[i].start <= tnow &&
                                    tnow < epg_full.entries[i].end) {
                                    epg_full_sel = (int)i;
                                    break;
                                }
                            }
                            epg_full_archive_id = archive_id;
                            snprintf(epg_full_channel_name,
                                     sizeof(epg_full_channel_name), "%s",
                                     pb->channel ? pb->channel->display : "");
                            epg_full_active = 1;
                        } else {
                            snprintf(toast_text, sizeof(toast_text),
                                     "Couldn't load EPG for this channel");
                            toast_until_ms = SDL_GetTicks() + 3000;
                        }
                    }
                } else {
                    show_overlay = !show_overlay;
                }
            }
            else if (k == SDLK_t) {
                static int always_on_top = 0;
                always_on_top = !always_on_top;
                SDL_SetWindowAlwaysOnTop(r.window, always_on_top ? SDL_TRUE : SDL_FALSE);
                snprintf(toast_text, sizeof(toast_text),
                         "Always on top: %s", always_on_top ? "ON" : "OFF");
                toast_until_ms = SDL_GetTicks() + 2500;
            }
            else if (k == SDLK_a) {
                /* Cycle audio tracks. Only makes sense when there's more
                 * than one — lots of live channels ship a single track, so
                 * the toast tells the user when there's nothing to switch. */
                int n = pb->player.n_audio_tracks;
                if (n <= 1) {
                    snprintf(toast_text, sizeof(toast_text),
                             "Only one audio track on this stream");
                } else {
                    int next = (pb->player.audio_track_cur + 1) % n;
                    player_set_audio_track(&pb->player, next);
                    const char *lang = pb->player.audio_lang[next];
                    snprintf(toast_text, sizeof(toast_text),
                             "Audio: %s (%d/%d)",
                             lang[0] ? lang : "track",
                             next + 1, n);
                }
                toast_until_ms = SDL_GetTicks() + 3000;
            }
            else if (k == SDLK_s) {
                /* Cycle subtitles: off -> track 0 -> track 1 -> ... -> off.
                 * Toast always shows what the user landed on (even "off")
                 * so the cycle is visible without looking at the video. */
                int n = pb->player.n_subtitle_tracks;
                if (n == 0) {
                    snprintf(toast_text, sizeof(toast_text),
                             "No subtitle tracks on this stream");
                } else {
                    int cur  = pb->player.subtitle_track_cur;  /* -1..n-1 */
                    int next = (cur + 2) % (n + 1) - 1;         /* cycles through -1..n-1 */
                    player_set_subtitle_track(&pb->player, next);
                    if (next < 0) {
                        snprintf(toast_text, sizeof(toast_text), "Subs: off");
                    } else {
                        const char *lang = pb->player.subtitle_lang[next];
                        snprintf(toast_text, sizeof(toast_text),
                                 "Subs: %s (%d/%d)",
                                 lang[0] ? lang : "track",
                                 next + 1, n);
                    }
                }
                toast_until_ms = SDL_GetTicks() + 3000;
            }
            else if ((k == SDLK_UP || k == SDLK_DOWN) && current_live_idx >= 0) {
                /* Same pipeline as mouse wheel: accumulate into pending_wheel_delta
                 * and let the WHEEL_DEBOUNCE_MS commit it through the async zap
                 * worker. Up arrow = previous channel (lower index), down = next. */
                pending_wheel_delta += (k == SDLK_UP) ? -1 : +1;
                last_wheel_ts = SDL_GetTicks();
            }
            else if (k == SDLK_LEFT || k == SDLK_RIGHT) {
                int delta = (k == SDLK_RIGHT) ? +30 : -30;
                if (pb->timeshift_start != 0) {
                    /* Catch-up / /timeshift/ replay — we rebuild the URL
                     * with a new start-time anchor and reopen playback.
                     * Can't use av_seek_frame here because the portal
                     * bakes start time into the URL path, not the stream. */
                    time_t new_start = pb->timeshift_start + delta;
                    char *u = xtream_timeshift_url(&portal, pb->stream_id, new_start,
                                                   pb->timeshift_dur_min);
                    playback_t *new_pb = calloc(1, sizeof(*new_pb));
                    if (u && new_pb && playback_open(new_pb, &r, pb->channel, u,
                                                     &portal, pb->stream_id) == 0) {
                        new_pb->timeshift_start   = new_start;
                        new_pb->timeshift_dur_min = pb->timeshift_dur_min;
                        playback_close(pb); free(pb); pb = new_pb;
                        set_window_title(&r, pb->channel->display);
                        paused = 0; have_texture = 0;
                        if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                        overlay_mark_dirty(&ov);
                        playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
                        last_audio_progress_ts = SDL_GetTicks();
                        prev_samples_played    = 0;
                        struct tm lt = *localtime(&new_start);
                        snprintf(toast_text, sizeof(toast_text),
                                 "%s 30s -> replay starts at %02d:%02d",
                                 delta > 0 ? "Skipped" : "Back", lt.tm_hour, lt.tm_min);
                    } else {
                        free(new_pb);
                        snprintf(toast_text, sizeof(toast_text),
                                 "%s 30s failed", delta > 0 ? "Skip" : "Back");
                    }
                    free(u);
                    toast_until_ms = SDL_GetTicks() + 4000;
                } else if (pb->player.fmt && pb->player.fmt->duration > 0) {
                    /* In-file seek for VOD / series episodes. Execution must
                     * happen under SDL_LockAudioDevice held for the ENTIRE
                     * seek: reset audio_out_t, drain queues, set seek_req,
                     * and spin-wait for the decoder to complete the seek +
                     * drain before releasing the lock. This makes the seek
                     * atomic from the audio callback's perspective: it
                     * wakes up after the lock is released with has_first_pts
                     * cleared and only POST-seek chunks in the queue, so the
                     * first chunk it pulls correctly seeds first_pts at the
                     * actual post-seek stream position (which may differ
                     * from target by a few seconds due to keyframe rounding
                     * under AVSEEK_FLAG_BACKWARD — letting the callback seed
                     * from reality is more robust than pre-seeding from
                     * target). Up to SEEK_WAIT_MS of user-visible lag. */
                    const int SEEK_WAIT_MS = 300;
                    double now_s    = av_clock_ready(&pb->clk) ? av_clock_now(&pb->clk) : 0.0;
                    double target_s = now_s + (double)delta;
                    if (target_s < 0) target_s = 0;
                    double max_s = (double)pb->player.fmt->duration / AV_TIME_BASE;
                    if (target_s > max_s - 1.0) target_s = max_s - 1.0;

                    SDL_LockAudioDevice(pb->audio.device);
                    if (pb->audio.cur) { audio_chunk_free(pb->audio.cur); pb->audio.cur = NULL; }
                    pb->audio.cur_samples    = NULL;
                    pb->audio.cur_remaining  = 0;
                    pb->audio.samples_played = 0;
                    pb->audio.has_first_pts  = 0;
                    pb->audio.first_pts      = 0;

                    queue_drain(&pb->player.audio_q, (void(*)(void*))audio_chunk_free);
                    queue_drain(&pb->player.video_q, (void(*)(void*))video_frame_free);
                    if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                    have_texture = 0;
                    overlay_mark_dirty(&ov);

                    pb->player.seek_target_pts = (int64_t)(target_s * AV_TIME_BASE);
                    pb->player.seek_req        = 1;

                    /* Spin-wait (callback is locked; user feels ~100-300ms
                     * of UI freeze on arrow-press). Decoder wakes from
                     * queue_push via the drain's not_full broadcast, sees
                     * seek_req, calls av_seek_frame, flushes, does its own
                     * drain to kill anything it pushed between our drain
                     * and its seek, clears seek_req. */
                    int waited = 0;
                    while (pb->player.seek_req && waited < SEEK_WAIT_MS) {
                        SDL_Delay(5);
                        waited += 5;
                    }
                    if (pb->player.seek_req)
                        fprintf(stderr, "[seek] decoder didn't ack in %dms — unlocking anyway\n",
                                SEEK_WAIT_MS);
                    else
                        fprintf(stderr, "[seek] decoder acked after %dms\n", waited);

                    SDL_UnlockAudioDevice(pb->audio.device);

                    last_audio_progress_ts = SDL_GetTicks();
                    prev_samples_played    = 0;
                    playback_open_ts       = SDL_GetTicks();  /* restart STARTUP_MS grace */

                    int mins = (int)(target_s / 60);
                    int secs = (int)target_s % 60;
                    snprintf(toast_text, sizeof(toast_text),
                             "%s 30s -> %d:%02d",
                             delta > 0 ? "Skipped" : "Back", mins, secs);
                    toast_until_ms = SDL_GetTicks() + 3000;
                } else {
                    snprintf(toast_text, sizeof(toast_text),
                             "Seeking is only supported on VOD + catch-up");
                    toast_until_ms = SDL_GetTicks() + 2500;
                }
            }
            else if (k == SDLK_QUESTION ||
                     (k == SDLK_SLASH && (ev.key.keysym.mod & KMOD_SHIFT))) {
                help_visible = !help_visible;
                hint_until_ms = 0;  /* startup hint has served its purpose */
            }
            else if (k == SDLK_n || k == SDLK_r) {
                /* Shared step-by-step news-programme search — 'n' is NOS
                 * Journaal across NPO 1/2/3, 'r' is RTL Nieuws across
                 * RTL 4/5/7/8/Z. Identical UX: toast updates per phase,
                 * single frame flushed between each blocking HTTP call. */
                static const news_programme_t PROG_NOS = {
                    .display_name  = "NOS Journaal",
                    .title_prefix  = "NOS Journaal",
                    .prefix_len    = 12,
                    .archive_ids   = XTREAM_NPO_ARCHIVE_STREAM_IDS,
                    .live_ids      = NULL,          /* NPO uses build_channel_url */
                    .channel_names = NULL,
                    .n_channels    = 3,
                };
                static const news_programme_t PROG_RTL = {
                    .display_name  = "RTL Nieuws",
                    .title_prefix  = "RTL Nieuws",
                    .prefix_len    = 10,
                    .archive_ids   = XTREAM_RTL_ARCHIVE_STREAM_IDS,
                    .live_ids      = XTREAM_RTL_LIVE_STREAM_IDS,
                    .channel_names = XTREAM_RTL_CHANNEL_NAMES,
                    .n_channels    = 5,
                };
                const news_programme_t *prog = (k == SDLK_n) ? &PROG_NOS : &PROG_RTL;

                SDL_Texture *cur_tex = have_texture ? pb->tex.texture : NULL;

                snprintf(toast_text, sizeof(toast_text),
                         "%s: scanning %d archive channel%s...",
                         prog->display_name, prog->n_channels,
                         prog->n_channels == 1 ? "" : "s");
                render_status_frame(&r, &ov, cur_tex, toast_text);

                /* Per-channel EPG fetch with toast updates between each. */
                epg_t *epgs = calloc((size_t)prog->n_channels, sizeof(epg_t));
                int   *jcount = calloc((size_t)prog->n_channels, sizeof(int));
                if (!epgs || !jcount) {
                    free(epgs); free(jcount);
                    snprintf(toast_text, sizeof(toast_text), "out of memory");
                    toast_until_ms = SDL_GetTicks() + 3000;
                    break;
                }
                const size_t pfxlen = prog->prefix_len;
                for (int i = 0; i < prog->n_channels; ++i) {
                    const char *chn = prog->channel_names
                                    ? prog->channel_names[i]
                                    : NPO_CHANNELS[i].display;
                    snprintf(toast_text, sizeof(toast_text),
                             "Step %d/%d: fetching %s archive EPG...",
                             i + 1, prog->n_channels, chn);
                    render_status_frame(&r, &ov, cur_tex, toast_text);
                    if (xtream_fetch_epg_full(&portal, prog->archive_ids[i], &epgs[i]) == 0) {
                        for (size_t j = 0; j < epgs[i].count; ++j) {
                            const char *t = epgs[i].entries[j].title;
                            if (t && strncasecmp(t, prog->title_prefix, pfxlen) == 0)
                                jcount[i]++;
                        }
                    }
                    snprintf(toast_text, sizeof(toast_text),
                             "%s: %d %s entries", chn, jcount[i], prog->display_name);
                    render_status_frame(&r, &ov, cur_tex, toast_text);
                }

                /* Pick the latest (airing > past > future) across the union. */
                time_t tnow = time(NULL);
                int best_airing_ch = -1, best_past_ch = -1, best_next_ch = -1;
                time_t best_airing_s = 0, best_airing_e = 0;
                time_t best_past_s   = 0, best_past_e   = 0;
                time_t best_next_s   = 0, best_next_e   = 0;
                for (int i = 0; i < prog->n_channels; ++i) {
                    for (size_t j = 0; j < epgs[i].count; ++j) {
                        const char *t = epgs[i].entries[j].title;
                        if (!t || strncasecmp(t, prog->title_prefix, pfxlen) != 0) continue;
                        time_t s = epgs[i].entries[j].start, e = epgs[i].entries[j].end;
                        if (s <= tnow && tnow < e) {
                            if (s > best_airing_s) {
                                best_airing_ch = i; best_airing_s = s; best_airing_e = e;
                            }
                        } else if (e <= tnow) {
                            if (s > best_past_s) {
                                best_past_ch = i; best_past_s = s; best_past_e = e;
                            }
                        } else {
                            if (best_next_ch < 0 || s < best_next_s) {
                                best_next_ch = i; best_next_s = s; best_next_e = e;
                            }
                        }
                    }
                }

                int ch_idx = -1;
                int is_airing = 0, is_past = 0;
                time_t hit_s = 0, hit_e = 0;
                if (best_airing_ch >= 0) {
                    ch_idx = best_airing_ch; is_airing = 1;
                    hit_s = best_airing_s; hit_e = best_airing_e;
                } else if (best_past_ch >= 0) {
                    ch_idx = best_past_ch; is_past = 1;
                    hit_s = best_past_s; hit_e = best_past_e;
                } else if (best_next_ch >= 0) {
                    ch_idx = best_next_ch;
                    hit_s = best_next_s; hit_e = best_next_e;
                }

                if (ch_idx < 0) {
                    snprintf(toast_text, sizeof(toast_text),
                             "No %s in any archive EPG", prog->display_name);
                } else {
                    struct tm lt = *localtime(&hit_s);
                    const char *chname = prog->channel_names
                                       ? prog->channel_names[ch_idx]
                                       : NPO_CHANNELS[ch_idx].display;
                    const char *kind = is_airing ? "airing NOW"
                                                 : (is_past ? "last ended"
                                                            : "next at");
                    snprintf(toast_text, sizeof(toast_text),
                             "Winner: %s %s %02d:%02d on %s",
                             prog->display_name, kind, lt.tm_hour, lt.tm_min, chname);
                    render_status_frame(&r, &ov, cur_tex, toast_text);

                    /* Build URL. Timeshift uses the archive id (replay);
                     * live/future paths need a "live" URL — for NPO that's
                     * build_channel_url, for RTL we fall back to the archive
                     * stream's live endpoint since the portal serves it OK. */
                    char *target_url      = NULL;
                    int   target_stream_id = 0;
                    if (is_airing || !is_past) {
                        if (prog->live_ids) {
                            /* Explicit LIVE stream IDs (e.g. RTL) — the archive
                             * IDs return 502 on /live/. */
                            target_stream_id = prog->live_ids[ch_idx];
                            target_url       = xtream_stream_url(&portal, target_stream_id);
                        } else {
                            /* NPO: use the existing NPO URL resolver. */
                            target_url = build_channel_url(ch_idx, &portal, direct_url);
                            target_stream_id = 0;
                        }
                        snprintf(toast_text, sizeof(toast_text),
                                 "Tuning live to %s%s...", chname,
                                 is_airing ? " for current broadcast" : " (wait for start)");
                    } else {
                        int dur_min = (int)((hit_e - hit_s) / 60);
                        target_stream_id = prog->archive_ids[ch_idx];
                        target_url = xtream_timeshift_url(&portal, target_stream_id,
                                                          hit_s, dur_min);
                        snprintf(toast_text, sizeof(toast_text),
                                 "Opening timeshift: /timeshift/.../%02d-%02d/%d.m3u8",
                                 lt.tm_hour, lt.tm_min, target_stream_id);
                    }
                    render_status_frame(&r, &ov, cur_tex, toast_text);

                    if (target_url) {
                        /* pb->channel must stay a valid npo_channel_t*; for RTL
                         * we stamp NPO_CHANNELS[0] as a vestigial holder. */
                        const npo_channel_t *target = (prog == &PROG_NOS)
                                                    ? &NPO_CHANNELS[ch_idx]
                                                    : &NPO_CHANNELS[0];
                        playback_t *new_pb = calloc(1, sizeof(*new_pb));
                        int epg_id = (is_past || prog == &PROG_RTL) ? target_stream_id : 0;
                        if (new_pb && playback_open(new_pb, &r, target, target_url,
                                                    &portal, epg_id) == 0) {
                            if (is_past) {
                                new_pb->timeshift_start   = hit_s;
                                new_pb->timeshift_dur_min = (int)((hit_e - hit_s) / 60);
                            }
                            playback_close(pb); free(pb); pb = new_pb;
                            set_window_title(&r, chname);
                            paused = 0; have_texture = 0;
                            if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                            overlay_mark_dirty(&ov);
                            playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
                            last_audio_progress_ts = SDL_GetTicks();
                            prev_samples_played    = 0;

                            if (is_past) {
                                long long ago_m = (tnow - hit_e) / 60;
                                snprintf(toast_text, sizeof(toast_text),
                                         "Replaying %s %02d:%02d on %s (ended %lldm ago)",
                                         prog->display_name, lt.tm_hour, lt.tm_min, chname, ago_m);
                            } else if (is_airing) {
                                long long in_m = (tnow - hit_s) / 60;
                                snprintf(toast_text, sizeof(toast_text),
                                         "%s airing on %s (started %02d:%02d, %lldm in)",
                                         prog->display_name, chname,
                                         lt.tm_hour, lt.tm_min, in_m);
                            } else {
                                long long til_m = (hit_s - tnow) / 60;
                                snprintf(toast_text, sizeof(toast_text),
                                         "Next %s at %02d:%02d on %s (in %lldm)",
                                         prog->display_name,
                                         lt.tm_hour, lt.tm_min, chname, til_m);
                            }
                        } else {
                            free(new_pb);
                            snprintf(toast_text, sizeof(toast_text),
                                     "Open failed for %s %s on %s",
                                     is_past ? "replay" : "live",
                                     prog->display_name, chname);
                        }
                        free(target_url);
                    }
                }
                for (int i = 0; i < prog->n_channels; ++i) npo_epg_free(&epgs[i]);
                free(epgs); free(jcount);
                toast_until_ms = SDL_GetTicks() + 8000;
            }
            else {
                const npo_channel_t *nch = key_to_channel(k);
                /* Same "force-on-zap" rule as the 'n' handler: if the user
                 * pressed 1/2/3 while zapped, pb->channel is a stale NPO stamp;
                 * use stream_id to detect that we're actually elsewhere. */
                if (nch && (nch != pb->channel || pb->stream_id != 0)) {
                    int ch_idx = (int)(nch - &NPO_CHANNELS[0]);
                    fprintf(stderr, "[switch] %s -> %s (idx=%d)\n",
                            pb->channel->display, nch->display, ch_idx);
                    char *switch_url = build_channel_url(ch_idx, &portal, direct_url);
                    /* Heap-allocate new_pb so its address is stable beyond this scope;
                     * pointer swap avoids the dangling-pointer bug that bit us on the
                     * first channel switch. */
                    playback_t *new_pb = calloc(1, sizeof(*new_pb));
                    if (new_pb && playback_open(new_pb, &r, nch, switch_url, &portal, 0) == 0) {
                        fprintf(stderr, "[switch] new playback opened, tearing down old\n");
                        playback_close(pb);
                        free(pb);
                        pb = new_pb;
                        set_window_title(&r, nch->display);
                        paused       = 0;
                        have_texture = 0;
                        if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                        overlay_mark_dirty(&ov);
                        playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
                        last_audio_progress_ts = SDL_GetTicks();
                        prev_samples_played    = 0;
                        fprintf(stderr, "[switch] swap complete, now on %s\n", pb->channel->display);
                    } else {
                        free(new_pb);
                        fprintf(stderr, "channel switch to %s failed - staying on %s\n",
                                nch->display, pb->channel->display);
                    }
                    free(switch_url);
                }
            }
        }

        /* 1b) Apply window drag if right-button is held. Skip when fullscreen —
         * moving a fullscreen window is meaningless and SDL_SetWindowPosition
         * on a fullscreen surface is undefined on some platforms. */
        if (drag_active && !r.fullscreen) {
            int mx, my;
            SDL_GetGlobalMouseState(&mx, &my);
            SDL_SetWindowPosition(r.window,
                drag_anchor_wx + (mx - drag_anchor_mx),
                drag_anchor_wy + (my - drag_anchor_my));
        }

        /* 1c) Debounced wheel-zap: flash channel name INSTANTLY, then spawn
         * a worker with its OWN heap-allocated zap_prep_t. Rapid scrolling
         * just detaches the old prep (which self-destructs) and allocates a
         * new one — no shared state to race on. */
        if (pending_wheel_delta != 0 && current_live_idx >= 0 &&
            SDL_GetTicks() - last_wheel_ts > WHEEL_DEBOUNCE_MS) {
            /* Remember the commit direction so state==4 auto-skip can step
             * PAST a dead channel in the same direction the user was zapping,
             * instead of stopping on "X unavailable" and making them press
             * up/down again. */
            if (pending_wheel_delta > 0) zap_direction = +1;
            else if (pending_wheel_delta < 0) zap_direction = -1;
            int new_idx = current_live_idx + pending_wheel_delta;
            if (new_idx < 0) new_idx = 0;
            if (new_idx >= (int)live_list.count) new_idx = (int)live_list.count - 1;
            pending_wheel_delta = 0;

            if (new_idx != current_live_idx) {
                xtream_live_entry_t *e = &live_list.entries[new_idx];
                fprintf(stderr, "[zap] [%d/%zu] %s (id=%d)\n",
                        new_idx + 1, live_list.count, e->name, e->stream_id);

                /* Abandon any prior prep — per-state handling avoids leaking
                 * a finished worker's result.
                 *   running(0) / EPG-ready(2) -> worker still alive; detach,
                 *     set self_owned+state=5, worker self-destructs.
                 *   stream-ready(3)           -> worker already exited; we
                 *     must join and destroy the pb we'll never show.
                 *   failed(4)                 -> worker already exited; join
                 *     and free the struct.
                 * Before this branch was a blanket detach, which leaked the
                 * prepared playback_t under fast-scroll. */
                if (zap_prep) {
                    int st = zap_prep->state;
                    if (st == 3) {
                        pthread_join(zap_prep->thread, NULL);
                        if (zap_prep->pb) { playback_close(zap_prep->pb); free(zap_prep->pb); }
                        free(zap_prep);
                    } else if (st == 4) {
                        pthread_join(zap_prep->thread, NULL);
                        free(zap_prep);
                    } else {
                        zap_prep->self_owned = 1;
                        zap_prep->state      = 5;
                        pthread_detach(zap_prep->thread);
                    }
                    zap_prep = NULL;
                }

                /* Fresh per-request struct. */
                zap_prep_t *zp = calloc(1, sizeof(*zp));
                if (zp) {
                    zp->portal        = &portal;
                    zp->render        = &r;
                    zp->url           = xtream_stream_url(&portal, e->stream_id);
                    zp->epg_stream_id = e->stream_id;
                    zp->channel       = pb->channel;
                    zp->list_idx      = new_idx;
                    snprintf(zp->label, sizeof(zp->label), "%s", e->name);

                    if (zp->url && pthread_create(&zp->thread, NULL,
                                                  zap_prep_worker, zp) == 0) {
                        zap_prep = zp;  /* track it */
                    } else {
                        free(zp->url); free(zp);
                    }
                }

                /* Phase-0 toast: channel name INSTANTLY — before the worker
                 * has even done anything. */
                snprintf(toast_text, sizeof(toast_text), "%s", e->name);
                toast_until_ms = SDL_GetTicks() + 8000;
            }
        }

        /* 1d) Phase 2: EPG arrived — enrich the toast. */
        if (zap_prep && zap_prep->state == 2 && !zap_prep->epg_toast_shown) {
            time_t tnow = time(NULL);
            const char *now_title = NULL;
            for (size_t i = 0; i < zap_prep->preload_epg.count; ++i) {
                if (zap_prep->preload_epg.entries[i].start <= tnow
                    && tnow < zap_prep->preload_epg.entries[i].end) {
                    now_title = zap_prep->preload_epg.entries[i].title;
                    break;
                }
            }
            if (now_title && *now_title) {
                snprintf(toast_text, sizeof(toast_text), "%s  |  %s",
                         zap_prep->label, now_title);
                toast_until_ms = SDL_GetTicks() + 8000;
            }
            zap_prep->epg_toast_shown = 1;
        }

        /* 1e) Phase 3: stream opened — swap playback. */
        if (zap_prep && zap_prep->state == 3) {
            if (!zap_prep->epg_toast_shown && zap_prep->pb) {
                time_t tnow = time(NULL);
                const char *nt = NULL;
                for (size_t i = 0; i < zap_prep->pb->epg.count; ++i) {
                    if (zap_prep->pb->epg.entries[i].start <= tnow
                        && tnow < zap_prep->pb->epg.entries[i].end) {
                        nt = zap_prep->pb->epg.entries[i].title; break;
                    }
                }
                if (nt && *nt)
                    snprintf(toast_text, sizeof(toast_text), "%s  |  %s", zap_prep->label, nt);
                zap_prep->epg_toast_shown = 1;
            }

            playback_t *new_pb = zap_prep->pb;
            pthread_join(zap_prep->thread, NULL);
            playback_close(pb); free(pb); pb = new_pb;
            current_live_idx = zap_prep->list_idx;
            zap_skip_count = 0;   /* got a working channel — reset the chain */
            paused = 0; have_texture = 0;
            if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
            overlay_mark_dirty(&ov);
            playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
            last_audio_progress_ts = SDL_GetTicks();
            prev_samples_played    = 0;

            set_window_title(&r, zap_prep->label);
            toast_until_ms = SDL_GetTicks() + 3000;

            free(zap_prep); zap_prep = NULL;
        } else if (zap_prep && zap_prep->state == 4) {
            pthread_join(zap_prep->thread, NULL);
            fprintf(stderr, "[zap] fail label=%s probe_failed=%d skip_count=%d\n",
                    zap_prep->label, zap_prep->probe_failed, zap_skip_count);
            /* Auto-advance past dead channels in the same direction the user
             * was zapping. Before this, one probe failure would park the
             * user on "X unavailable" and make them press the arrow again
             * — and if the NEXT channel was also dead, same thing, so
             * zapping through a patch of dead channels felt broken.
             *
             * Only advance on probe_failed (definite 403/502/timeout) — a
             * generic open-failure could mean the channel IS reachable but
             * libav rejected it, which we don't want to chain past. Bound
             * the chain with ZAP_SKIP_LIMIT so a completely-broken portal
             * doesn't zap the user to the end of the catalog silently. */
            if (zap_prep->probe_failed && zap_direction != 0 &&
                zap_skip_count < ZAP_SKIP_LIMIT) {
                zap_skip_count++;
                /* CRITICAL: advance current_live_idx past the failed slot
                 * before re-issuing delta. Otherwise next commit computes
                 * new_idx = current(still 49) + delta(+1) = 50 = same dead
                 * channel, and we infinite-loop. Advancing to list_idx
                 * makes delta move PAST the failed entry. Visually
                 * current_live_idx briefly points to a dead slot until
                 * the next successful state==3 resets it, but the
                 * displayed video stays on the last-known-good pb — no
                 * visible glitch, just a number briefly off. */
                current_live_idx = zap_prep->list_idx;
                pending_wheel_delta = zap_direction;
                last_wheel_ts = SDL_GetTicks() - WHEEL_DEBOUNCE_MS - 1;
                snprintf(toast_text, sizeof(toast_text),
                         "%s unavailable — trying next (%d)",
                         zap_prep->label, zap_skip_count);
            } else if (zap_prep->probe_failed) {
                snprintf(toast_text, sizeof(toast_text),
                         "%s is unavailable — stopped after %d tries",
                         zap_prep->label, zap_skip_count);
                zap_skip_count = 0;
            } else {
                snprintf(toast_text, sizeof(toast_text),
                         "Tuning to %s failed", zap_prep->label);
            }
            toast_until_ms = SDL_GetTicks() + 3000;
            free(zap_prep); zap_prep = NULL;
        }

        /* Audio liveness tick. */
        Uint32 tnow_ms = SDL_GetTicks();
        int64_t cur_samples = pb->audio.samples_played;
        if (cur_samples != prev_samples_played) {
            prev_samples_played    = cur_samples;
            last_audio_progress_ts = tnow_ms;
        }

        /* Post-seek verification. After the decoder completes a seek it
         * records the first post-seek audio pts in seek_verify_actual_s.
         * If that value is far from target, the portal didn't honour our
         * Range request — the stream is still playing from the pre-seek
         * position. Our pre-seeded clock (anchored at target) now
         * disagrees with the actual stream pts, which would drop every
         * video frame. Rebase first_pts to match actual so audio+video
         * stay consistent at the real position, and toast the user. */
        if (!pb->player.seek_req && pb->player.seek_verify_actual_s >= 0.0) {
            double target = pb->player.seek_verify_target_s;
            double actual = pb->player.seek_verify_actual_s;
            double drift  = actual - target;
            if (drift < -5.0 || drift > 5.0) {
                SDL_LockAudioDevice(pb->audio.device);
                pb->audio.first_pts = actual -
                                      (double)pb->audio.samples_played /
                                      (double)pb->audio.sample_rate;
                SDL_UnlockAudioDevice(pb->audio.device);
                fprintf(stderr, "[seek] portal ignored Range (drift=%.1fs) — "
                                "clock rebased to actual pts=%.1fs\n", drift, actual);
                snprintf(toast_text, sizeof(toast_text),
                         "Seek not supported by portal — skipping only works on catch-up");
                toast_until_ms = SDL_GetTicks() + 4500;
            }
            /* One-shot check: mark as consumed. */
            pb->player.seek_verify_actual_s = -1.0;
            pb->player.seek_verify_target_s = 0.0;
        }

        /* Audio warmup: unpause the SDL audio device as soon as the decoder
         * has pushed at least one video frame, OR after AUDIO_WARMUP_MAX_MS
         * (audio-only streams / broken video decoders). Without this, every
         * zap swap had audio starting ~300-800ms before video; the clock
         * would seed from audio's head-start and later a video-resync would
         * rebase it — producing persistent A/V desync even after the
         * resync "corrected" the clock. By starting audio paused and only
         * unpausing when video is ready, first_pts seeds at the moment both
         * streams genuinely begin. */
        if (!audio_warmed && !paused) {
            if (pb->player.video_frames_pushed > 0 ||
                tnow_ms - playback_open_ts > AUDIO_WARMUP_MAX_MS) {
                SDL_PauseAudioDevice(pb->audio.device, 0);
                audio_warmed = 1;
                fprintf(stderr, "[warmup] audio unpaused at t=%ums "
                                "(vframes=%lld)\n",
                        (unsigned)(tnow_ms - playback_open_ts),
                        pb->player.video_frames_pushed);
            }
        }

        /* Heartbeat — liveness ping. Override interval when the test harness
         * env var is set so we get fine-grained logs around the auto-seek. */
        Uint32 hb_interval = (test_hb_ms > 0) ? (Uint32)test_hb_ms : HEARTBEAT_MS;
        if (tnow_ms - last_heartbeat >= hb_interval) {
            fprintf(stderr, "[heartbeat] t=%.2fs vq=%zu aq=%zu samples=%lld clk=%.1f vframes=%lld aframes=%lld have_tex=%d\n",
                    (SDL_GetTicks() - test_launch_ms) / 1000.0,
                    pb->player.video_q.count, pb->player.audio_q.count,
                    (long long)pb->audio.samples_played,
                    av_clock_ready(&pb->clk) ? av_clock_now(&pb->clk) : 0.0,
                    pb->player.video_frames_pushed, pb->player.audio_frames_pushed,
                    have_texture);
            last_heartbeat = tnow_ms;
        }

        /* Test-harness triggers: auto-seek + auto-exit. */
        if (autoseek_at > 0 && !autoseek_fired &&
            (tnow_ms - test_launch_ms) / 1000.0 >= autoseek_at) {
            SDL_Event e = {0};
            e.type = SDL_KEYDOWN;
            e.key.keysym.sym = SDLK_RIGHT;
            e.key.keysym.mod = 0;
            SDL_PushEvent(&e);
            fprintf(stderr, "[test] injected SDLK_RIGHT at t=%.2fs (clock=%.2fs)\n",
                    (tnow_ms - test_launch_ms) / 1000.0,
                    av_clock_ready(&pb->clk) ? av_clock_now(&pb->clk) : 0.0);
            autoseek_fired = 1;
        }
        if (test_exit_at > 0 && (tnow_ms - test_launch_ms) / 1000.0 >= test_exit_at) {
            fprintf(stderr, "[test] exit deadline at t=%.2fs\n",
                    (tnow_ms - test_launch_ms) / 1000.0);
            running = 0;
        }
        if (autozap_count > 0 && autozap_fired < autozap_count) {
            double t_now = (tnow_ms - test_launch_ms) / 1000.0;
            double t_expected = autozap_start + autozap_fired * autozap_spacing_s;
            if (t_now >= t_expected) {
                SDL_Event e = {0};
                e.type = SDL_KEYDOWN;
                e.key.keysym.sym = SDLK_DOWN;
                SDL_PushEvent(&e);
                autozap_fired++;
                fprintf(stderr, "[test] injected SDLK_DOWN #%d at t=%.2fs (current_live_idx=%d)\n",
                        autozap_fired, t_now, current_live_idx);
            }
        }

        /* 2) Try to grab a new frame if we don't have one pending. */
        if (!pending_vf) pending_vf = queue_try_pop(&pb->player.video_q);
        /* NB: last_frame_ts is NOT updated here anymore. It used to update
         * whenever we held a frame, which meant a stuck clock (audio callback
         * dead -> clock frozen -> frame held forever) kept the stall detector
         * from ever firing. Now we only update it on successful PRESENT, so
         * a genuine presentation freeze triggers the stall-restart after
         * STALL_MS. */

        /* 3) If we have a pending frame, decide what to do with it.
         * Skip the decide-when block until audio has actually started
         * (first chunk consumed by SDL) — without that baseline av_clock
         * returns 0 and every frame with a real pts looks "late". */
        if (pending_vf && av_clock_ready(&pb->clk)) {
            double now  = av_clock_now(&pb->clk);
            double diff = pending_vf->pts - now;

            /* Startup A/V resync. On VOD mp4 the H.264 decoder needs to see
             * an IDR before producing its first frame, which often lags
             * 300-800ms behind the first audio chunk. Audio seeds the
             * clock immediately, samples_played advances with wall-time,
             * and when the first video frame finally arrives its pts is
             * "that far behind" the clock — so DROP_LATE throws it away,
             * and every subsequent frame too (because pts advance at
             * real-time rate while the clock keeps running). The whole
             * movie plays as audio-only.
             *
             * Fix: before we've ever displayed a frame, if the first one
             * looks past-due by more than DROP_LATE, rebase the clock so
             * THIS frame is due now. We can safely overwrite first_pts
             * because the audio callback only writes it once (on the
             * first chunk) and only reads it during that write — after
             * has_first_pts is set, the clock value is purely a function
             * of (first_pts, samples_played), both of which main may
             * modify without coordinating with the callback further.
             *
             * BUT cap the resync at STARTUP_RESYNC_MAX_S. Anything beyond
             * that is almost certainly a stale pre-seek frame that slipped
             * past main's drain: the decoder was blocked in queue_push
             * when main drained, got unblocked by the drain signal, and
             * pushed its blocked-packet-worth of data onto the now-empty
             * queue BEFORE it iterated and saw seek_req. That stale frame
             * carries the OLD pts and rebasing to it traps every valid
             * post-seek frame in "future" limbo (diff > 0, held forever,
             * queue fills, decoder blocks, black screen). Discard-and-wait
             * is safer: a valid post-seek frame will arrive within a few
             * hundred ms. */
            if (!have_texture && -diff > DROP_LATE) {
                double new_first = pending_vf->pts -
                                   (double)pb->audio.samples_played /
                                   (double)pb->audio.sample_rate;
                pb->audio.first_pts = new_first;
                fprintf(stderr, "[sync] VOD startup resync: clock "
                                "rebased to video pts=%.2fs (audio was %.2fs ahead)\n",
                        pending_vf->pts, -diff);
                now  = av_clock_now(&pb->clk);
                diff = pending_vf->pts - now;
            }

            if (diff < DUE_WINDOW) {
                /* Due now (or past due). Either upload-for-display, or drop if way late. */
                if (-diff > DROP_LATE) {
                    video_frame_free(pending_vf);
                } else {
                    video_tex_upload(&pb->tex, r.renderer, pending_vf);
                    video_frame_free(pending_vf);
                    have_texture = 1;
                }
                pending_vf = NULL;
            }
            /* else: hold for next iteration — its time hasn't come yet. */
        }

        /* 3b) Stall detection + auto-restart. Two trigger conditions:
         *
         *  (a) audio WAS progressing but stopped (samples_played stuck for
         *      STALL_MS) — SDL's audio callback died mid-playback, or the
         *      decoder stopped pushing chunks because av_read_frame is
         *      wedged on a dead TCP connection.
         *
         *  (b) audio NEVER started (has_first_pts==0) more than STARTUP_MS
         *      after playback_open returned. This catches the "open succeeded
         *      but first-packet probe never emitted a usable frame" path, which
         *      presents as a permanent black window with no audio — genuinely
         *      indistinguishable from a crash to the user.
         *
         * The have_texture gate was removed in 2026-04-18 after diagnosing a
         * 150s test failure: MPEG-TS pts desync dropped every video frame, so
         * have_texture stayed 0 forever, so the stall detector never ran, so
         * the player froze silently. have_texture wasn't protecting anything —
         * the AUDIO progress check is the real signal of liveness. */
        int startup_elapsed = (int)(SDL_GetTicks() - playback_open_ts);
        int audio_stalled = pb->audio.has_first_pts &&
                            SDL_GetTicks() - last_audio_progress_ts > STALL_MS;
        int never_started = !pb->audio.has_first_pts &&
                            startup_elapsed > (int)STARTUP_MS;
        /* Skip stall-restart on finite sources (VOD, series episodes). They
         * don't have a "restart URL" in the channel sense — pb->stream_id
         * is 0 and pb->timeshift_start is 0, so the existing fallback would
         * build build_channel_url(ch_idx=0) = NPO 1, which is exactly the
         * bug the user hit after pressing right-arrow on a movie: seek +
         * HTTP rebuffer took > STARTUP_MS, stall fired, the player
         * "restarted" onto NPO 1. For VOD the right behavior is to wait
         * out the rebuffer; if the file is truly broken the decoder will
         * hit EOF or av_read_frame failure and audio will silently stop —
         * user can press q or zap to move on. */
        int is_vod = pb->timeshift_start == 0 && pb->stream_id == 0 &&
                     pb->player.fmt && pb->player.fmt->duration > 0;
        if (!paused && !is_vod && (audio_stalled || never_started)) {
            if (never_started)
                fprintf(stderr, "[stall] audio never started after %dms — restarting\n",
                        startup_elapsed);
            char *restart_url;
            int   restart_epg_id;
            if (pb->timeshift_start != 0 && portal.host) {
                /* Timeshift replay: resume at the same start/duration so we
                 * don't randomly switch to the archive stream's live feed. */
                restart_url = xtream_timeshift_url(&portal, pb->stream_id,
                                                   pb->timeshift_start,
                                                   pb->timeshift_dur_min);
                restart_epg_id = pb->stream_id;
                fprintf(stderr, "[stall] timeshift id=%d start=%ld restarting\n",
                        pb->stream_id, (long)pb->timeshift_start);
            } else if (pb->stream_id != 0 && portal.host) {
                restart_url    = xtream_stream_url(&portal, pb->stream_id);
                restart_epg_id = pb->stream_id;
                fprintf(stderr, "[stall] no frames for %ums on portal stream_id=%d - restarting\n",
                        STALL_MS, pb->stream_id);
            } else {
                int ch_idx = (int)(pb->channel - &NPO_CHANNELS[0]);
                restart_url    = build_channel_url(ch_idx, &portal, direct_url);
                restart_epg_id = 0;
                fprintf(stderr, "[stall] no frames for %ums on %s - restarting\n",
                        STALL_MS, pb->channel->display);
            }
            playback_t *new_pb = calloc(1, sizeof(*new_pb));
            if (new_pb && playback_open(new_pb, &r, pb->channel, restart_url, &portal, restart_epg_id) == 0) {
                /* Preserve timeshift state across restart. */
                if (pb->timeshift_start != 0) {
                    new_pb->timeshift_start   = pb->timeshift_start;
                    new_pb->timeshift_dur_min = pb->timeshift_dur_min;
                }
                playback_close(pb);
                free(pb);
                pb = new_pb;
                paused       = 0;
                have_texture = 0;
                if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                overlay_mark_dirty(&ov);
                playback_open_ts = SDL_GetTicks(); audio_warmed = 0;
                fprintf(stderr, "[stall] restart OK\n");
            } else {
                free(new_pb);
                fprintf(stderr, "[stall] restart failed - will retry in %ums\n", STALL_MS);
            }
            free(restart_url);
            /* Reset the audio-progress tracker so we don't fire again 3s later. */
            last_audio_progress_ts = SDL_GetTicks();
            prev_samples_played    = 0;
        }

        /* 4) Always render. VSYNC in SDL_RenderPresent paces the loop to ~60Hz,
         *    so holding the last frame costs nothing and keeps the window alive
         *    even when new frames haven't arrived. */
        SDL_RenderClear(r.renderer);
        int ww = 0, wh = 0;
        SDL_GetRendererOutputSize(r.renderer, &ww, &wh);
        if (have_texture) {
            SDL_RenderCopy(r.renderer, pb->tex.texture, NULL, NULL);
            if (show_overlay) {
                /* Reference time: for timeshift replay we're showing content
                 * from pb->timeshift_start + (time-since-playback-began).
                 * Approximation: timeshift_start + samples_played/sample_rate.
                 * For live streams, wallclock is correct. */
                time_t ref = 0;
                if (pb->timeshift_start != 0 && pb->audio.sample_rate > 0) {
                    ref = pb->timeshift_start +
                          (time_t)(pb->audio.samples_played / pb->audio.sample_rate);
                }
                overlay_render(&ov, r.renderer, &pb->epg, ref, ww, wh);
            }
        }
        /* Help + search + toast + startup hint on top of everything so they
         * stay legible. Priority: search > help > toast > startup hint.
         * Search takes top priority because the user is actively typing and
         * needs to see their query + results without other overlays stealing
         * attention. */
        if (epg_full_active) {
            /* Multi-day EPG list. Labels: "Vandaag 18:00  NOS Journaal".
             * Past entries dimmed visually would be nice but we keep one
             * color for now — the highlight row plus the day label is
             * enough orientation. Window recenters around selection so
             * scrolling a 800-entry list still shows you what you just
             * arrowed to. */
            static char epg_labels[32][256];
            const char *epg_names[32];
            int nshow = (int)epg_full.count < SEARCH_VISIBLE
                        ? (int)epg_full.count : SEARCH_VISIBLE;
            int start_i = epg_full_sel - nshow / 2;
            if (start_i < 0) start_i = 0;
            if (start_i + nshow > (int)epg_full.count)
                start_i = (int)epg_full.count - nshow;
            if (start_i < 0) start_i = 0;
            time_t tnow = time(NULL);
            for (int i = 0; i < nshow; ++i) {
                epg_entry_t *e = &epg_full.entries[start_i + i];
                struct tm lt_s = *localtime(&e->start);
                char daybuf[32];
                relative_day_label(e->start, tnow, daybuf, sizeof(daybuf));
                const char *marker = "   ";
                if (e->end <= tnow)       marker = "   ";  /* past */
                else if (e->start <= tnow) marker = "NU ";  /* airing */
                else                       marker = "-> ";  /* future */
                snprintf(epg_labels[i], sizeof(epg_labels[i]),
                         "%s%-9s %02d:%02d  %s",
                         marker, daybuf,
                         lt_s.tm_hour, lt_s.tm_min,
                         e->title ? e->title : "");
                epg_names[i] = epg_labels[i];
            }
            char hdr[280];
            snprintf(hdr, sizeof(hdr),
                     "EPG — %s  (Enter=play, up/down/PgUp/PgDn, Esc=close)",
                     epg_full_channel_name);
            overlay_render_search(&ov, r.renderer, hdr, epg_names, nshow,
                                  epg_full_sel - start_i, ww, wh);
        } else if (episode_picker_active) {
            /* Episode list overlay — same visual as the search overlay so
             * the user sees one consistent UI. We build "S#E# - Title"
             * labels into a scratch buffer and pass pointers in. */
            static char ep_labels[32][192];
            const char *ep_names[32];
            int nshow = (int)episodes.count < SEARCH_VISIBLE
                        ? (int)episodes.count : SEARCH_VISIBLE;
            /* Center the window around the selection so a long episode
             * list still keeps the chosen row in view. */
            int start_i = episode_sel - nshow / 2;
            if (start_i < 0) start_i = 0;
            if (start_i + nshow > (int)episodes.count)
                start_i = (int)episodes.count - nshow;
            if (start_i < 0) start_i = 0;
            for (int i = 0; i < nshow; ++i) {
                xtream_episode_entry_t *ep = &episodes.entries[start_i + i];
                snprintf(ep_labels[i], sizeof(ep_labels[i]), "S%02dE%02d  %s",
                         ep->season_num, ep->episode_num,
                         ep->title ? ep->title : "");
                ep_names[i] = ep_labels[i];
            }
            char title[280];
            snprintf(title, sizeof(title), "%s  —  Enter to play, Esc to go back",
                     episode_series_name);
            overlay_render_search(&ov, r.renderer, title, ep_names, nshow,
                                  episode_sel - start_i, ww, wh);
        } else if (search_active) {
            static char labels[32][256];
            const char *names[32];
            int nshow = search_hits_count < SEARCH_VISIBLE ? search_hits_count : SEARCH_VISIBLE;
            for (int i = 0; i < nshow; ++i) {
                search_hit_label(&search_hits[i], &live_list, &vod_list, &series_list,
                                 labels[i], sizeof(labels[i]));
                names[i] = labels[i];
            }
            char hdr[256];
            snprintf(hdr, sizeof(hdr), "Search:  %s_", search_query);
            overlay_render_search(&ov, r.renderer, hdr, names,
                                  nshow, search_sel, ww, wh);
        } else if (help_visible) {
            overlay_render_help(&ov, r.renderer, ww, wh);
        } else if (SDL_GetTicks() < toast_until_ms && toast_text[0]) {
            overlay_render_hint(&ov, r.renderer, toast_text, ww, wh);
        } else if (SDL_GetTicks() < hint_until_ms) {
            overlay_render_hint(&ov, r.renderer, "Press ? for help", ww, wh);
        }

        /* Subtitle rendering. Read under player_t.sub_mu so the decoder
         * can't free the string mid-TTF_Render. Only display while the
         * clock is within the current entry's time window. */
        if (pb->player.subtitle_track_cur >= 0 && pb->player.sub_mu_init &&
            av_clock_ready(&pb->clk)) {
            double now_s = av_clock_now(&pb->clk);
            pthread_mutex_lock(&pb->player.sub_mu);
            if (pb->player.sub_text &&
                now_s >= pb->player.sub_start && now_s < pb->player.sub_end) {
                overlay_render_subtitle(&ov, r.renderer, pb->player.sub_text, ww, wh);
            }
            pthread_mutex_unlock(&pb->player.sub_mu);
        }

        SDL_RenderPresent(r.renderer);  /* blocks ~16ms @ 60Hz with VSYNC */
    }

    /* On quit, hand the in-flight prep to the worker to clean itself up.
     * Detach rather than join — worker could be mid-network and joining
     * would stall the exit by up to ~10s. */
    if (zap_prep) {
        zap_prep->self_owned = 1;
        zap_prep->state      = 5;
        pthread_detach(zap_prep->thread);
        zap_prep = NULL;
    }

    if (pending_vf) video_frame_free(pending_vf);
    playback_close(pb);
    free(pb);
    free(search_hits);
    npo_epg_free(&epg_full);
    xtream_episodes_free(&episodes);
    overlay_shutdown(&ov);
    render_shutdown(&r);
    xtream_live_list_free(&live_list);
    xtream_vod_list_free(&vod_list);
    xtream_series_list_free(&series_list);
    xtream_free(&portal);
    curl_global_cleanup();
    return 0;
}
