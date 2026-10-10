#ifndef TV_PLAYER_H
#define TV_PLAYER_H

#include "common.h"
#include "hls_prefetch.h"
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>
#include <pthread.h>

#define PLAYER_MAX_AUDIO_TRACKS 8
#define PLAYER_MAX_SUBTITLE_TRACKS 8

typedef struct {
    AVFormatContext  *fmt;
    int               video_idx;
    int               audio_idx;   /* current AV stream index (one of audio_tracks[]) */
    AVCodecContext   *vctx;
    AVCodecContext   *actx;
    struct SwsContext *sws;         /* to YUV420P if needed */
    struct SwrContext *swr;         /* to s16 stereo 48k */

    queue_t           video_q;      /* holds video_frame_t* */
    queue_t           audio_q;      /* holds audio_chunk_t* */

    hls_prefetch_t   *prefetch;   /* HLS live prefetcher; NULL for non-HLS */

    pthread_t         thread;
    volatile int      stop;

    /* Watchdog state, written by the decoder thread, read by the audio
     * callback and by the main-loop watchdog. Not a synchronization
     * primitive — just "did libav give up?" and "when did we last make
     * forward network progress?". Used to detect silent decoder death
     * that silence-fill in the audio callback would otherwise mask
     * forever (see audio_callback in render.c and the watchdog branch
     * in main.c). */
    volatile int          decoder_done;          /* 1 after EOF / error break */
    volatile unsigned int decoder_last_read_ms;  /* SDL_GetTicks() of last ok av_read_frame */

    int               audio_sample_rate_out;
    long long         video_frames_pushed;
    long long         audio_frames_pushed;

    /* Multi-track audio. Dubbed VOD files commonly ship with 2-3 audio PIDs
     * (FR, NL, EN), and the default pick is "first stream found" — which is
     * how "No Time to Die (NL)" ended up playing the French dub track.
     *
     * The decoder thread watches audio_track_req; when it differs from
     * audio_track_cur it flushes the current audio codec context, reopens
     * it for the new stream, and drains audio_q so the user doesn't hear
     * 2s of the old language tailing off. swr_context gets rebuilt too
     * because the new stream can have a different sample-rate or channel
     * layout. */
    int               audio_tracks[PLAYER_MAX_AUDIO_TRACKS];  /* AV stream indices */
    char              audio_lang[PLAYER_MAX_AUDIO_TRACKS][16]; /* "nld"/"fra"/... or "" */
    int               n_audio_tracks;
    int               audio_track_cur;  /* index into audio_tracks[] the decoder is ON */
    volatile int      audio_track_req;  /* index into audio_tracks[] the UI wants */

    /* Seek request: main sets seek_req=1 and seek_target_pts to the absolute
     * stream pts (in AV_TIME_BASE units) to seek to, then drains the A/V
     * queues and resets audio_out_t. Decoder thread picks up seek_req at
     * the top of its next iteration and calls av_seek_frame. Only sensible
     * for finite streams (VOD, series episodes) — live streams silently
     * no-op because their containers advertise duration=0 and av_seek_frame
     * returns an error we ignore. */
    volatile int      seek_req;
    volatile int64_t  seek_target_pts;   /* in AV_TIME_BASE units */

    /* Post-seek diagnostics / range-not-supported detection. Some portals
     * don't honour HTTP Range requests — av_seek_frame reports success
     * (byte offset sent in header) but the server returns 200 OK from
     * byte 0, leaving the demuxer reading pre-seek packets. Detect by
     * recording the pts of the first packet decoded after a seek and
     * comparing to target. seek_verify_pending is set on seek, cleared
     * by the decoder after verifying (or giving up). */
    volatile int      seek_verify_pending;
    volatile double   seek_verify_target_s;
    volatile double   seek_verify_actual_s;

    /* Subtitle tracks. Same request/current volatile pair as audio, with
     * -1 meaning "subs off" and 0..n_subtitle_tracks-1 meaning a specific
     * track. Only text-shaped subtitles are supported for rendering
     * (subrip, mov_text, webvtt, ass, text). Bitmap subs (PGS, DVD, DVB)
     * decode but we can't blit them — the toast warns the user. Decoded
     * text lives in sub_* under sub_mu and is displayed by main while
     * the clock is within [sub_start, sub_end). */
    int               subtitle_tracks[PLAYER_MAX_SUBTITLE_TRACKS];
    char              subtitle_lang[PLAYER_MAX_SUBTITLE_TRACKS][16];
    int               n_subtitle_tracks;
    int               subtitle_track_cur;   /* -1 = off */
    volatile int      subtitle_track_req;   /* -1 = off */
    AVCodecContext   *sctx;                 /* subtitle decoder (or NULL) */

    pthread_mutex_t   sub_mu;
    int               sub_mu_init;          /* guards sub_mu destroy */
    char             *sub_text;             /* malloc'd, NULL = no active sub */
    double            sub_start;            /* in stream-relative seconds */
    double            sub_end;
} player_t;

int  player_open(player_t *p, const char *url);
void player_close(player_t *p);
int  player_start(player_t *p);            /* spawns decoder thread */
void player_stop(player_t *p);             /* signals stop, joins */

/* Request the decoder thread switch to audio_tracks[track_idx]. track_idx
 * is 0..n_audio_tracks-1. Returns 0 on a valid request, -1 out of range.
 * The switch is asynchronous; audio audibly changes after the current
 * audio queue drains (~300ms typical). */
int  player_set_audio_track(player_t *p, int track_idx);

/* Request the decoder thread switch to subtitle_tracks[track_idx], or
 * turn subs off with track_idx = -1. The decoder reopens sctx for the
 * new stream on its next loop iteration and clears any pending text. */
int  player_set_subtitle_track(player_t *p, int track_idx);

#endif
