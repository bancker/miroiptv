#include "player.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Callback that libav polls during blocking I/O (open, read). Returning non-zero
 * aborts the current operation. We return p->stop so that setting p->stop=1
 * from another thread can break av_read_frame out of a network read. */
static int io_interrupt_cb(void *opaque) {
    player_t *p = opaque;
    return p->stop;
}

int player_open(player_t *p, const char *url) {
    memset(p, 0, sizeof(*p));
    p->video_idx = -1;
    p->audio_idx = -1;
    p->audio_sample_rate_out = 48000;

    /* Pre-allocate the AVFormatContext so we can set the interrupt callback
     * BEFORE avformat_open_input makes its first network call (otherwise a
     * hostile/slow URL could wedge this function indefinitely). */
    p->fmt = avformat_alloc_context();
    if (!p->fmt) { fprintf(stderr, "avformat_alloc_context failed\n"); return -1; }
    p->fmt->interrupt_callback.callback = io_interrupt_cb;
    p->fmt->interrupt_callback.opaque   = p;

    /* HTTP / HLS reconnect tuning. Live streams drop connections routinely —
     * without these flags libav surrenders on the first hiccup and the
     * decoder thread exits, freezing the player.
     *
     * NOTE: rw_timeout removed — a 10s cap aborted HLS playlist refreshes on
     * slow hops, which caused the decoder to exit after one playlist (~30-60s
     * of content). Let libav's reconnect handle real failures; io_interrupt_cb
     * lets us still cancel the thread on shutdown. */
    AVDictionary *opts = NULL;
    av_dict_set(&opts, "reconnect",                  "1", 0);
    av_dict_set(&opts, "reconnect_streamed",         "1", 0);
    av_dict_set(&opts, "reconnect_on_network_error", "1", 0);
    av_dict_set(&opts, "reconnect_on_http_error",    "404,403,5xx", 0);
    av_dict_set(&opts, "reconnect_delay_max",        "5", 0);  /* seconds */
    /* NB: reconnect_at_eof is NOT set. For finite resources (timeshift .ts
     * files) it caused libav to re-download the whole file after EOF and
     * the second download's packets confused the MPEG-TS demuxer — video
     * froze after ~60 s while audio kept playing. Let EOF be EOF. */
    av_dict_set(&opts, "reconnect_max_retries",      "10", 0);
    /* 2 MiB TCP recv buffer gives libav headroom to absorb bursty CDN
     * delivery without blocking the write side, reducing the chance the
     * server closes the connection for slow reads. */
    av_dict_set(&opts, "buffer_size",                "2097152", 0);
    /* HLS-specific (harmless for raw .ts): keep HTTP connection alive across
     * segment fetches, start near live edge. */
    av_dict_set(&opts, "http_persistent",            "1", 0);
    av_dict_set(&opts, "live_start_index",           "-1", 0);
    /* Probe / analyze generously so mid-stream opens (timeshift, catch-up
     * replay) find the H.264 SPS/PPS before we call avformat_find_stream_info
     * done. Defaults (5 MB / 5 s) often miss the first keyframe when opened
     * at an arbitrary wall-clock offset, producing audio-only playback with
     * "non-existing PPS 0 referenced" warnings forever. */
    av_dict_set(&opts, "probesize",                  "32000000", 0);   /* 32 MB */
    av_dict_set(&opts, "analyzeduration",            "15000000", 0);   /* 15 sec in us */
    /* Tolerate bitstream hiccups around the mid-stream entry point rather
     * than hard-failing the decode context. */
    av_dict_set(&opts, "fflags",                     "+discardcorrupt+genpts", 0);

    int rc_open = avformat_open_input(&p->fmt, url, NULL, &opts);
    av_dict_free(&opts);
    if (rc_open != 0) {
        fprintf(stderr, "avformat_open_input failed for %s\n", url);
        goto fail;
    }
    if (avformat_find_stream_info(p->fmt, NULL) < 0) {
        fprintf(stderr, "avformat_find_stream_info failed\n");
        goto fail;
    }

    for (unsigned i = 0; i < p->fmt->nb_streams; ++i) {
        AVStream *s = p->fmt->streams[i];
        if (s->codecpar->codec_type == AVMEDIA_TYPE_VIDEO && p->video_idx < 0)
            p->video_idx = (int)i;
        else if (s->codecpar->codec_type == AVMEDIA_TYPE_AUDIO && p->audio_idx < 0)
            p->audio_idx = (int)i;
    }
    if (p->video_idx < 0 || p->audio_idx < 0) {
        fprintf(stderr, "need both video and audio streams (v=%d a=%d)\n",
                p->video_idx, p->audio_idx);
        goto fail;
    }

    /* Open video decoder */
    AVCodecParameters *vpar = p->fmt->streams[p->video_idx]->codecpar;
    const AVCodec *vc = avcodec_find_decoder(vpar->codec_id);
    if (!vc) { fprintf(stderr, "no decoder for video codec id %d\n", vpar->codec_id); goto fail; }
    p->vctx = avcodec_alloc_context3(vc);
    avcodec_parameters_to_context(p->vctx, vpar);
    if (avcodec_open2(p->vctx, vc, NULL) < 0) goto fail;

    /* Open audio decoder */
    AVCodecParameters *apar = p->fmt->streams[p->audio_idx]->codecpar;
    const AVCodec *ac = avcodec_find_decoder(apar->codec_id);
    if (!ac) { fprintf(stderr, "no decoder for audio codec id %d\n", apar->codec_id); goto fail; }
    p->actx = avcodec_alloc_context3(ac);
    avcodec_parameters_to_context(p->actx, apar);
    if (avcodec_open2(p->actx, ac, NULL) < 0) goto fail;

    if (queue_init(&p->video_q, 16) != 0) goto fail;
    if (queue_init(&p->audio_q, 32) != 0) goto fail;
    return 0;

fail:
    /* Undo any partial init. player_close is safe on a half-initialized
     * player_t because every cleanup is NULL/empty-checked (and queue_destroy
     * guards against a never-initialized queue). */
    player_close(p);
    return -1;
}

void player_close(player_t *p) {
    if (p->sws) sws_freeContext(p->sws);
    if (p->swr) swr_free(&p->swr);
    if (p->vctx) avcodec_free_context(&p->vctx);
    if (p->actx) avcodec_free_context(&p->actx);
    if (p->fmt)  avformat_close_input(&p->fmt);
    queue_destroy_with(&p->video_q, (void(*)(void*))video_frame_free);
    queue_destroy_with(&p->audio_q, (void(*)(void*))audio_chunk_free);
    memset(p, 0, sizeof(*p));
}

static void alloc_frame_copy_yuv(const AVFrame *in, video_frame_t *out) {
    out->width  = in->width;
    out->height = in->height;
    out->stride_y = in->width;
    out->stride_u = in->width / 2;
    out->stride_v = in->width / 2;
    out->y = malloc((size_t)out->stride_y * in->height);
    out->u = malloc((size_t)out->stride_u * in->height / 2);
    out->v = malloc((size_t)out->stride_v * in->height / 2);
    for (int r = 0; r < in->height; ++r)
        memcpy(out->y + r * out->stride_y, in->data[0] + r * in->linesize[0], out->stride_y);
    for (int r = 0; r < in->height / 2; ++r)
        memcpy(out->u + r * out->stride_u, in->data[1] + r * in->linesize[1], out->stride_u);
    for (int r = 0; r < in->height / 2; ++r)
        memcpy(out->v + r * out->stride_v, in->data[2] + r * in->linesize[2], out->stride_v);
}

static double ts_to_seconds(int64_t pts, AVRational tb) {
    if (pts == AV_NOPTS_VALUE) return 0.0;
    return (double)pts * tb.num / tb.den;
}

/* Seconds since the stream's first packet. Different PIDs in an MPEG-TS can
 * advertise wildly different absolute pts origins (observed: audio ~8876s vs
 * video ~2630s on NPO1 live), because the demuxer records each stream's first
 * pts in stream->start_time and the playback container expects consumers to
 * subtract it. Without this normalization, av_clock (seeded from audio pts)
 * sits at 8876s while video frames arrive at 2630s, so every video frame
 * looks 6246s late and gets DROP_LATE'd — audio-only playback forever. */
static double stream_rel_seconds(int64_t pts, AVStream *s) {
    if (pts == AV_NOPTS_VALUE) return 0.0;
    int64_t rel = pts;
    if (s->start_time != AV_NOPTS_VALUE) rel -= s->start_time;
    return (double)rel * s->time_base.num / s->time_base.den;
}

static void push_video_frame(player_t *p, AVFrame *frame) {
    if (frame->format != AV_PIX_FMT_YUV420P) {
        if (!p->sws) {
            p->sws = sws_getContext(frame->width, frame->height, frame->format,
                                    frame->width, frame->height, AV_PIX_FMT_YUV420P,
                                    SWS_BILINEAR, NULL, NULL, NULL);
        }
        AVFrame *conv = av_frame_alloc();
        conv->format = AV_PIX_FMT_YUV420P;
        conv->width  = frame->width;
        conv->height = frame->height;
        av_frame_get_buffer(conv, 32);
        sws_scale(p->sws, (const uint8_t *const *)frame->data, frame->linesize,
                  0, frame->height, conv->data, conv->linesize);
        conv->pts = frame->pts;
        video_frame_t *vf = calloc(1, sizeof(*vf));
        alloc_frame_copy_yuv(conv, vf);
        vf->pts = stream_rel_seconds(conv->pts, p->fmt->streams[p->video_idx]);
        av_frame_free(&conv);
        if (queue_push(&p->video_q, vf) != 0) video_frame_free(vf);
        return;
    }
    video_frame_t *vf = calloc(1, sizeof(*vf));
    alloc_frame_copy_yuv(frame, vf);
    vf->pts = stream_rel_seconds(frame->pts, p->fmt->streams[p->video_idx]);
    if (queue_push(&p->video_q, vf) != 0) video_frame_free(vf);
}

static void push_audio_frame(player_t *p, AVFrame *frame) {
    if (!p->swr) {
        AVChannelLayout out_layout = AV_CHANNEL_LAYOUT_STEREO;
        swr_alloc_set_opts2(&p->swr,
            &out_layout, AV_SAMPLE_FMT_S16, p->audio_sample_rate_out,
            &p->actx->ch_layout, p->actx->sample_fmt, p->actx->sample_rate,
            0, NULL);
        swr_init(p->swr);
    }
    int max_out_samples = (int)av_rescale_rnd(
        swr_get_delay(p->swr, p->actx->sample_rate) + frame->nb_samples,
        p->audio_sample_rate_out, p->actx->sample_rate, AV_ROUND_UP);
    int16_t *out_buf = malloc((size_t)max_out_samples * 2 * sizeof(int16_t));
    uint8_t *out_ptrs[1] = { (uint8_t *)out_buf };
    int written = swr_convert(p->swr, out_ptrs, max_out_samples,
                              (const uint8_t **)frame->extended_data, frame->nb_samples);
    if (written <= 0) { free(out_buf); return; }
    audio_chunk_t *ac = calloc(1, sizeof(*ac));
    ac->samples     = out_buf;
    ac->n_samples   = written;
    ac->sample_rate = p->audio_sample_rate_out;
    ac->pts         = stream_rel_seconds(frame->pts, p->fmt->streams[p->audio_idx]);
    if (queue_push(&p->audio_q, ac) != 0) audio_chunk_free(ac);
}

static void *decoder_loop(void *ud) {
    player_t *p = ud;
    AVPacket *pkt = av_packet_alloc();
    AVFrame  *fr  = av_frame_alloc();

    while (!p->stop) {
        int rc = av_read_frame(p->fmt, pkt);
        if (rc < 0) {
            char errbuf[AV_ERROR_MAX_STRING_SIZE] = {0};
            av_strerror(rc, errbuf, sizeof(errbuf));
            if (rc == AVERROR_EOF) {
                fprintf(stderr, "[decoder] EOF (rc=AVERROR_EOF) — stream ended\n");
            } else {
                fprintf(stderr, "[decoder] av_read_frame failed: %s (rc=%d)\n", errbuf, rc);
            }
            break;
        }
        AVCodecContext *ctx = NULL;
        int is_video = 0;
        if (pkt->stream_index == p->video_idx) { ctx = p->vctx; is_video = 1; }
        else if (pkt->stream_index == p->audio_idx) { ctx = p->actx; }
        if (!ctx) { av_packet_unref(pkt); continue; }

        /* Don't skip packets here — the MPEG-TS demuxer interleaves SPS/PPS
         * NAL units with the slice NALs they describe, and skipping any
         * packet can strand the decoder without parameter sets. Let the
         * H.264 decoder emit "non-existing PPS" warnings until it picks up
         * in-band headers; that's normal for mid-stream opens. */
        if (avcodec_send_packet(ctx, pkt) == 0) {
            while (avcodec_receive_frame(ctx, fr) == 0) {
                if (is_video) {
                    push_video_frame(p, fr);
                    p->video_frames_pushed++;
                    if (p->video_frames_pushed == 1)
                        fprintf(stderr, "[decoder] first video frame decoded (%dx%d, rel_pts=%.2fs, abs_pts=%.2fs)\n",
                                fr->width, fr->height,
                                stream_rel_seconds(fr->pts, p->fmt->streams[p->video_idx]),
                                ts_to_seconds(fr->pts, p->fmt->streams[p->video_idx]->time_base));
                } else {
                    push_audio_frame(p, fr);
                    p->audio_frames_pushed++;
                    if (p->audio_frames_pushed == 1)
                        fprintf(stderr, "[decoder] first audio frame decoded (rel_pts=%.2fs, abs_pts=%.2fs)\n",
                                stream_rel_seconds(fr->pts, p->fmt->streams[p->audio_idx]),
                                ts_to_seconds(fr->pts, p->fmt->streams[p->audio_idx]->time_base));
                }
                av_frame_unref(fr);
            }
        }
        av_packet_unref(pkt);
    }

    queue_close(&p->video_q);
    queue_close(&p->audio_q);
    av_frame_free(&fr);
    av_packet_free(&pkt);
    return NULL;
}

int player_start(player_t *p) {
    p->stop = 0;
    return pthread_create(&p->thread, NULL, decoder_loop, p);
}

void player_stop(player_t *p) {
    p->stop = 1;
    queue_close(&p->video_q);
    queue_close(&p->audio_q);
    pthread_join(p->thread, NULL);
}
