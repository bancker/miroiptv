#include "player.h"
#include <stdio.h>
#include <string.h>

int player_open(player_t *p, const char *url) {
    memset(p, 0, sizeof(*p));
    p->video_idx = -1;
    p->audio_idx = -1;
    p->audio_sample_rate_out = 48000;

    if (avformat_open_input(&p->fmt, url, NULL, NULL) != 0) {
        fprintf(stderr, "avformat_open_input failed for %s\n", url);
        return -1;
    }
    if (avformat_find_stream_info(p->fmt, NULL) < 0) {
        fprintf(stderr, "avformat_find_stream_info failed\n");
        return -1;
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
        return -1;
    }

    /* Open video decoder */
    AVCodecParameters *vpar = p->fmt->streams[p->video_idx]->codecpar;
    const AVCodec *vc = avcodec_find_decoder(vpar->codec_id);
    if (!vc) { fprintf(stderr, "no decoder for video codec id %d\n", vpar->codec_id); return -1; }
    p->vctx = avcodec_alloc_context3(vc);
    avcodec_parameters_to_context(p->vctx, vpar);
    if (avcodec_open2(p->vctx, vc, NULL) < 0) return -1;

    /* Open audio decoder */
    AVCodecParameters *apar = p->fmt->streams[p->audio_idx]->codecpar;
    const AVCodec *ac = avcodec_find_decoder(apar->codec_id);
    if (!ac) { fprintf(stderr, "no decoder for audio codec id %d\n", apar->codec_id); return -1; }
    p->actx = avcodec_alloc_context3(ac);
    avcodec_parameters_to_context(p->actx, apar);
    if (avcodec_open2(p->actx, ac, NULL) < 0) return -1;

    if (queue_init(&p->video_q, 16) != 0) return -1;
    if (queue_init(&p->audio_q, 32) != 0) return -1;
    return 0;
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

/* player_start / player_stop are stubs in this task; next task fills them. */
int  player_start(player_t *p) { (void)p; return 0; }
void player_stop(player_t *p)  { (void)p; }
