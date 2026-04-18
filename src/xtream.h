#ifndef TV_XTREAM_H
#define TV_XTREAM_H

#include "npo.h"  /* epg_t, epg_entry_t reused from the NPO module */

/* Xtream Codes portal credentials. host/user/pass are malloc'd strings owned
 * by the struct; port is an integer (portal's HTTP port, typically 8080). */
typedef struct {
    char *host;
    int   port;
    char *user;
    char *pass;
} xtream_t;

/* Parses "user:pass@host[:port]" (port defaults to 8080). Returns 0 on
 * success and populates *out; caller must xtream_free it. Returns -1 on
 * malformed input. */
int  xtream_parse(const char *spec, xtream_t *out);
void xtream_free(xtream_t *x);

/* Builds an HLS manifest URL for the given Xtream Codes stream id.
 * Returns a malloc'd string; caller frees. */
char *xtream_stream_url(const xtream_t *x, int stream_id);

/* Per-channel stream IDs for this portal (m.hnlol.com). HD variants chosen
 * for sensible-quality default. Index matches NPO_CHANNELS[] in npo.h. */
extern const int XTREAM_NPO_STREAM_IDS[3];

/* Fetches the short EPG (upcoming programs) for a given stream id via the
 * portal's player_api.php?action=get_short_epg endpoint. Titles arrive
 * base64-encoded; we decode them and flag NOS Journaal entries the same way
 * the NPO parser does. Returns 0 on success, -1 on failure. Caller frees
 * with npo_epg_free (same struct). */
int xtream_fetch_epg(const xtream_t *x, int stream_id, epg_t *out);

/* One entry in the portal's live-stream catalog. */
typedef struct {
    int   stream_id;
    int   num;         /* portal's display order */
    char *name;        /* malloc'd, free via xtream_live_list_free */
} xtream_live_entry_t;

typedef struct {
    xtream_live_entry_t *entries;
    size_t               count;
} xtream_live_list_t;

/* Fetches the full live-stream catalog (all channels the account has access
 * to). Typically ~15000 entries. category_id == NULL means "all categories".
 * Returns 0 on success. */
int  xtream_fetch_live_list(const xtream_t *x, const char *category_id,
                            xtream_live_list_t *out);
void xtream_live_list_free(xtream_live_list_t *list);

#endif
