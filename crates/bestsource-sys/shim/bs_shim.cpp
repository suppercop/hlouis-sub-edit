// extern "C" shim over BestSource. Part of hlouis-sub-edit, BSD-3-Clause.
// BestSource itself is MIT (Copyright (c) 2022-2024 Fredrik Mellbin), see licenses/BestSource-MIT.txt.

#include "bs_shim.h"

#include <cstdio>
#include <cstring>
#include <exception>
#include <memory>
#include <string>

#include "audiosource.h"
#include "bsshared.h"
#include "tracklist.h"
#include "version.h"
#include "videosource.h"

extern "C" {
#include <libavutil/frame.h>
#include <libavutil/log.h>
#include <libavutil/pixfmt.h>
#include <libswscale/swscale.h>
}

namespace {

void SetErr(char *err, size_t len, const char *msg) {
    if (!err || len == 0)
        return;
    std::snprintf(err, len, "%s", msg ? msg : "unknown error");
}

struct ProgressCtx {
    bs_progress_cb Cb;
    void *User;
    bool Cancelled = false;
};

ProgressFunction MakeProgress(ProgressCtx &Ctx) {
    if (!Ctx.Cb)
        return nullptr;
    return [&Ctx](int Track, int64_t Current, int64_t Total) -> bool {
        if (Ctx.Cb(Ctx.User, Track, Current, Total) != 0) {
            Ctx.Cancelled = true;
            return false;
        }
        return true;
    };
}

std::filesystem::path CachePathOf(const char *CachePath) {
    if (!CachePath || !*CachePath)
        return {};
    return CreateProbablyUTF8Path(CachePath);
}

int ThreadsOf(int Threads) {
    return Threads > 0 ? Threads : 0;
}

/* Runs Fn, mapping every exception to an error code + message. */
template <typename F>
int Guard(char *err, size_t len, F &&Fn) {
    try {
        return Fn();
    } catch (const BestSourceCancelledException &e) {
        SetErr(err, len, e.what());
        return BS_CANCELLED;
    } catch (const std::exception &e) {
        SetErr(err, len, e.what());
        return BS_ERR;
    } catch (...) {
        SetErr(err, len, "unknown C++ exception");
        return BS_ERR;
    }
}

int SwsColorspace(const AVFrame *F) {
    switch (F->colorspace) {
    case AVCOL_SPC_BT709:
        return SWS_CS_ITU709;
    case AVCOL_SPC_FCC:
        return SWS_CS_FCC;
    case AVCOL_SPC_BT470BG:
    case AVCOL_SPC_SMPTE170M:
        return SWS_CS_ITU601;
    case AVCOL_SPC_SMPTE240M:
        return SWS_CS_SMPTE240M;
    case AVCOL_SPC_BT2020_NCL:
    case AVCOL_SPC_BT2020_CL:
        return SWS_CS_BT2020;
    default:
        return F->height > 576 ? SWS_CS_ITU709 : SWS_CS_ITU601;
    }
}

} // namespace

struct bs_video {
    std::unique_ptr<BestVideoSource> Src;
    SwsContext *Sws = nullptr;
    int SwsW = 0, SwsH = 0, SwsFmt = -1, SwsCs = -1, SwsRange = -1;
    bs_cancel_cb CancelCb = nullptr;
    void *CancelUser = nullptr;
    ~bs_video() { sws_freeContext(Sws); }
};

struct bs_audio {
    std::unique_ptr<BestAudioSource> Src;
    bs_cancel_cb CancelCb = nullptr;
    void *CancelUser = nullptr;
};

extern "C" int bs_version(void) {
    return BEST_SOURCE_VERSION_MAJOR * 100 + BEST_SOURCE_VERSION_MINOR;
}

extern "C" void bs_set_ffmpeg_log_level(int level) {
    SetFFmpegLogLevel(level);
}

extern "C" int bs_track_list(const char *path_utf8, int *media_types, int cap, char *err, size_t err_len) {
    return Guard(err, err_len, [&]() -> int {
        BestTrackList TL(CreateProbablyUTF8Path(path_utf8), nullptr);
        int N = TL.GetNumTracks();
        if (media_types) {
            for (int i = 0; i < N && i < cap; i++)
                media_types[i] = TL.GetTrackInfo(i).MediaType;
        }
        return N;
    });
}

extern "C" int bs_video_open(const char *path_utf8, int track, int threads, int cache_mode,
                             const char *cache_path_utf8, bs_progress_cb progress, void *progress_user,
                             bs_video **out, char *err, size_t err_len) {
    if (!out || !path_utf8) {
        SetErr(err, err_len, "null argument");
        return BS_ERR;
    }
    *out = nullptr;
    ProgressCtx Ctx{progress, progress_user};
    int R = Guard(err, err_len, [&]() -> int {
        auto V = std::make_unique<bs_video>();
        V->Src = std::make_unique<BestVideoSource>(CreateProbablyUTF8Path(path_utf8), false, std::nullopt, track, 0,
                                                   ThreadsOf(threads), cache_mode, CachePathOf(cache_path_utf8),
                                                   nullptr, MakeProgress(Ctx));
        *out = V.release();
        return BS_OK;
    });
    if (R != BS_OK && Ctx.Cancelled) {
        SetErr(err, err_len, "indexing cancelled");
        return BS_CANCELLED;
    }
    return R;
}

extern "C" void bs_video_close(bs_video *v) {
    delete v;
}

extern "C" void bs_video_set_cancel(bs_video *v, bs_cancel_cb cb, void *user) {
    if (!v)
        return;
    v->CancelCb = cb;
    v->CancelUser = user;
    if (cb)
        v->Src->SetCancellationCallback([v]() { return v->CancelCb && v->CancelCb(v->CancelUser) != 0; });
    else
        v->Src->SetCancellationCallback(nullptr);
}

extern "C" void bs_video_get_props(const bs_video *v, bs_video_props *o) {
    if (!v || !o)
        return;
    const BSVideoProperties &P = v->Src->GetVideoProperties();
    o->track = v->Src->GetTrack();
    o->width = P.Width;
    o->height = P.Height;
    o->num_frames = P.NumFrames;
    o->fps_num = P.FPS.Num;
    o->fps_den = P.FPS.Den;
    o->time_base_num = P.TimeBase.Num;
    o->time_base_den = P.TimeBase.Den;
    o->duration = P.Duration;
    o->start_time = P.StartTime;
    o->pix_fmt = P.Format;
    o->color_family = P.VF.ColorFamily;
    o->bits = P.VF.Bits;
    o->sub_sampling_w = P.VF.SubSamplingW;
    o->sub_sampling_h = P.VF.SubSamplingH;
}

extern "C" int bs_video_frame_table(const bs_video *v, int64_t *pts, uint8_t *keyframe, int64_t count) {
    if (!v)
        return BS_ERR;
    try {
        int64_t N = v->Src->GetVideoProperties().NumFrames;
        if (count > N)
            count = N;
        for (int64_t i = 0; i < count; i++) {
            const BestVideoSource::FrameInfo &FI = v->Src->GetFrameInfo(i);
            if (pts)
                pts[i] = FI.PTS;
            if (keyframe)
                keyframe[i] = FI.KeyFrame ? 1 : 0;
        }
        return BS_OK;
    } catch (...) {
        return BS_ERR;
    }
}

extern "C" int bs_video_get_frame_rgba(bs_video *v, int64_t n, uint8_t *dst, ptrdiff_t stride, int width, int height,
                                       int64_t *pts_out, char *err, size_t err_len) {
    if (!v || !dst) {
        SetErr(err, err_len, "null argument");
        return BS_ERR;
    }
    return Guard(err, err_len, [&]() -> int {
        std::unique_ptr<BestVideoFrame> Frame(v->Src->GetFrame(n));
        if (!Frame)
            throw BestSourceException("failed to decode frame " + std::to_string(n));
        const AVFrame *F = Frame->GetAVFrame();
        if (Frame->Width != width || Frame->Height != height)
            throw BestSourceException("frame " + std::to_string(n) + " is " + std::to_string(Frame->Width) + "x" +
                                      std::to_string(Frame->Height) + ", expected " + std::to_string(width) + "x" +
                                      std::to_string(height));
        int Cs = SwsColorspace(F);
        int Range = F->color_range == AVCOL_RANGE_JPEG ? 1 : 0;
        if (!v->Sws || v->SwsW != width || v->SwsH != height || v->SwsFmt != F->format || v->SwsCs != Cs ||
            v->SwsRange != Range) {
            sws_freeContext(v->Sws);
            v->Sws = sws_getContext(width, height, static_cast<AVPixelFormat>(F->format), width, height,
                                    AV_PIX_FMT_RGBA, SWS_FAST_BILINEAR, nullptr, nullptr, nullptr);
            if (!v->Sws)
                throw BestSourceException("sws_getContext failed");
            sws_setColorspaceDetails(v->Sws, sws_getCoefficients(Cs), Range, sws_getCoefficients(SWS_CS_DEFAULT), 1, 0,
                                     1 << 16, 1 << 16);
            v->SwsW = width;
            v->SwsH = height;
            v->SwsFmt = F->format;
            v->SwsCs = Cs;
            v->SwsRange = Range;
        }
        uint8_t *Dst[4] = {dst, nullptr, nullptr, nullptr};
        int DstStride[4] = {static_cast<int>(stride), 0, 0, 0};
        int H = sws_scale(v->Sws, F->data, F->linesize, 0, height, Dst, DstStride);
        if (H != height)
            throw BestSourceException("sws_scale failed");
        if (pts_out)
            *pts_out = Frame->PTS;
        return BS_OK;
    });
}

extern "C" int bs_video_get_frame_planar(bs_video *v, int64_t n, uint8_t *const dst[3], const ptrdiff_t stride[3],
                                         char *err, size_t err_len) {
    if (!v || !dst || !stride) {
        SetErr(err, err_len, "null argument");
        return BS_ERR;
    }
    return Guard(err, err_len, [&]() -> int {
        std::unique_ptr<BestVideoFrame> Frame(v->Src->GetFrame(n));
        if (!Frame)
            throw BestSourceException("failed to decode frame " + std::to_string(n));
        uint8_t *D[3] = {dst[0], dst[1] ? dst[1] : dst[0], dst[2] ? dst[2] : dst[0]};
        ptrdiff_t S[3] = {stride[0], stride[1], stride[2]};
        if (!Frame->ExportAsPlanar(D, S))
            throw BestSourceException("frame " + std::to_string(n) + " has an unsupported pixel format");
        return BS_OK;
    });
}

extern "C" int bs_audio_open(const char *path_utf8, int track, int threads, int cache_mode,
                             const char *cache_path_utf8, bs_progress_cb progress, void *progress_user,
                             bs_audio **out, char *err, size_t err_len) {
    if (!out || !path_utf8) {
        SetErr(err, err_len, "null argument");
        return BS_ERR;
    }
    *out = nullptr;
    ProgressCtx Ctx{progress, progress_user};
    int R = Guard(err, err_len, [&]() -> int {
        auto A = std::make_unique<bs_audio>();
        A->Src = std::make_unique<BestAudioSource>(CreateProbablyUTF8Path(path_utf8), track, -1, ThreadsOf(threads),
                                                   cache_mode, CachePathOf(cache_path_utf8), nullptr, 0.0,
                                                   MakeProgress(Ctx));
        *out = A.release();
        return BS_OK;
    });
    if (R != BS_OK && Ctx.Cancelled) {
        SetErr(err, err_len, "indexing cancelled");
        return BS_CANCELLED;
    }
    return R;
}

extern "C" void bs_audio_close(bs_audio *a) {
    delete a;
}

extern "C" void bs_audio_set_cancel(bs_audio *a, bs_cancel_cb cb, void *user) {
    if (!a)
        return;
    a->CancelCb = cb;
    a->CancelUser = user;
    if (cb)
        a->Src->SetCancellationCallback([a]() { return a->CancelCb && a->CancelCb(a->CancelUser) != 0; });
    else
        a->Src->SetCancellationCallback(nullptr);
}

extern "C" void bs_audio_get_props(const bs_audio *a, bs_audio_props *o) {
    if (!a || !o)
        return;
    const BSAudioProperties &P = a->Src->GetAudioProperties();
    o->track = a->Src->GetTrack();
    o->sample_rate = P.SampleRate;
    o->channels = P.Channels;
    o->bits_per_sample = P.BitsPerSample;
    o->bytes_per_sample = P.AF.BytesPerSample;
    o->is_float = P.AF.Float ? 1 : 0;
    o->num_samples = P.NumSamples;
    o->num_frames = P.NumFrames;
    o->start_time = P.StartTime;
    o->channel_layout = P.ChannelLayout;
}

extern "C" int bs_audio_get_packed(bs_audio *a, uint8_t *dst, int64_t start, int64_t count, char *err,
                                   size_t err_len) {
    if (!a || !dst) {
        SetErr(err, err_len, "null argument");
        return BS_ERR;
    }
    return Guard(err, err_len, [&]() -> int {
        a->Src->GetPackedAudio(dst, start, count);
        return BS_OK;
    });
}
