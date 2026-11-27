// extern "C" shim over BestSource. Part of hlouis-sub-edit, BSD-3-Clause.
// BestSource itself is MIT (Copyright (c) 2022-2024 Fredrik Mellbin), see licenses/BestSource-MIT.txt.

#include "bs_shim.h"

#include <cstdio>
#include <cstring>
#include <algorithm>
#include <exception>
#include <map>
#include <memory>
#include <string>

#include "audiosource.h"
#include "bsshared.h"
#include "tracklist.h"
#include "version.h"
#include "videosource.h"

extern "C" {
#include <libavformat/avformat.h>
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

/* Cached swscale context converting decoded frames to RGBA8 with the frame's own matrix. */
class RgbaConverter {
    SwsContext *Sws = nullptr;
    int W = 0, H = 0, Fmt = -1, Cs = -1, Range = -1;

public:
    RgbaConverter() = default;
    RgbaConverter(const RgbaConverter &) = delete;
    RgbaConverter &operator=(const RgbaConverter &) = delete;
    ~RgbaConverter() { sws_freeContext(Sws); }

    void Convert(const AVFrame *F, int Width, int Height, uint8_t *Dst, ptrdiff_t Stride) {
        int NewCs = SwsColorspace(F);
        int NewRange = F->color_range == AVCOL_RANGE_JPEG ? 1 : 0;
        if (!Sws || W != Width || H != Height || Fmt != F->format || Cs != NewCs || Range != NewRange) {
            sws_freeContext(Sws);
            Sws = sws_getContext(Width, Height, static_cast<AVPixelFormat>(F->format), Width, Height, AV_PIX_FMT_RGBA,
                                 SWS_FAST_BILINEAR, nullptr, nullptr, nullptr);
            if (!Sws)
                throw BestSourceException("sws_getContext failed");
            sws_setColorspaceDetails(Sws, sws_getCoefficients(NewCs), NewRange, sws_getCoefficients(SWS_CS_DEFAULT), 1,
                                     0, 1 << 16, 1 << 16);
            W = Width;
            H = Height;
            Fmt = F->format;
            Cs = NewCs;
            Range = NewRange;
        }
        uint8_t *D[4] = {Dst, nullptr, nullptr, nullptr};
        int DS[4] = {static_cast<int>(Stride), 0, 0, 0};
        if (sws_scale(Sws, F->data, F->linesize, 0, Height, D, DS) != Height)
            throw BestSourceException("sws_scale failed");
    }
};

} // namespace

struct bs_video {
    std::unique_ptr<BestVideoSource> Src;
    RgbaConverter Conv;
    bs_cancel_cb CancelCb = nullptr;
    void *CancelUser = nullptr;
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
        v->Conv.Convert(F, width, height, dst, stride);
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

/* ---------------------------------------------------------------------------------------------- */
/* Quick, index-free video                                                                          */
/* ---------------------------------------------------------------------------------------------- */

struct bs_quick {
    std::unique_ptr<LWVideoDecoder> Dec;
    LWVideoProperties VP = {};
    AVFrame *Last = nullptr; /* the frame returned for slot LastN */
    int64_t LastN = -1;
    int64_t FirstPTS = 0;
    int Width = 0;
    int Height = 0;
    int Track = -1;
    RgbaConverter Conv;
    ~bs_quick() { av_frame_free(&Last); }

    static int64_t PtsOf(const AVFrame *F) {
        if (F->pts != AV_NOPTS_VALUE)
            return F->pts;
        return F->best_effort_timestamp;
    }

    /* Slot (CFR frame number) of a timestamp, rounded to nearest. */
    int64_t SlotOf(int64_t Pts) const {
        if (Pts == AV_NOPTS_VALUE)
            return LastN + 1;
        const long double Num = static_cast<long double>(Pts - FirstPTS) * VP.TimeBase.Num * VP.FPS.Num;
        const long double Den = static_cast<long double>(VP.TimeBase.Den) * VP.FPS.Den;
        return static_cast<int64_t>(Num / Den + (Num >= 0 ? 0.5L : -0.5L));
    }

    int64_t PtsOfSlot(int64_t N) const {
        const long double T = static_cast<long double>(N) * VP.FPS.Den * VP.TimeBase.Den /
                              (static_cast<long double>(VP.FPS.Num) * VP.TimeBase.Num);
        return FirstPTS + static_cast<int64_t>(T);
    }

    void Keep(AVFrame *F, int64_t N) {
        av_frame_free(&Last);
        Last = F;
        LastN = N;
    }

    /* Decodes forward until a frame at slot >= N; returns false at end of stream. */
    bool DecodeForwardTo(int64_t N) {
        while (true) {
            AVFrame *F = Dec->GetNextFrame();
            if (!F)
                return false;
            int64_t S = SlotOf(PtsOf(F));
            if (S >= N) {
                Keep(F, S);
                return true;
            }
            av_frame_free(&F);
        }
    }

    void Get(int64_t N) {
        if (Last && LastN == N)
            return;
        /* Forward within a short distance: just keep decoding. */
        if (Last && N > LastN && N - LastN <= 48) {
            DecodeForwardTo(N); /* at end of stream the last decoded frame is kept */
            return;
        }
        const int64_t BackStep = 10 * (VP.FPS.Num / std::max(1, VP.FPS.Den) + 1);
        int64_t Target = PtsOfSlot(N);
        for (int Attempt = 0; Attempt < 3; Attempt++) {
            if (!Dec->Seek(Target))
                break;
            AVFrame *F = Dec->GetNextFrame();
            if (!F)
                break;
            int64_t S = SlotOf(PtsOf(F));
            if (S > N && N > 0 && Attempt < 2) {
                /* Landed after the target (sparse cues or open GOP): seek further back. */
                av_frame_free(&F);
                Target = PtsOfSlot(std::max<int64_t>(0, N - (Attempt + 1) * BackStep));
                continue;
            }
            Keep(F, S);
            if (S < N)
                DecodeForwardTo(N);
            return;
        }
        throw BestSourceException("seek failed");
    }
};

extern "C" int bs_quick_open(const char *path_utf8, int track, int threads, bs_quick **out, char *err, size_t err_len) {
    if (!out || !path_utf8) {
        SetErr(err, err_len, "null argument");
        return BS_ERR;
    }
    *out = nullptr;
    return Guard(err, err_len, [&]() -> int {
        auto Q = std::make_unique<bs_quick>();
        std::map<std::string, std::string> Opts;
        Q->Dec = std::make_unique<LWVideoDecoder>(CreateProbablyUTF8Path(path_utf8), false, std::nullopt, track, 0,
                                                  ThreadsOf(threads), Opts, nullptr);
        Q->Dec->GetVideoProperties(Q->VP);
        Q->Track = Q->Dec->GetTrack();
        {
            /* The container header (no stream probing, so this is cheap) has the frame rate as
               the muxer wrote it (the codec context can report e.g. 24000/1 for MPEG-4 ASP) and,
               for Matroska, the only duration (there is no per-stream one). */
            AVFormatContext *Ctx = nullptr;
            if (avformat_open_input(&Ctx, path_utf8, nullptr, nullptr) == 0) {
                if (Q->Track >= 0 && Q->Track < static_cast<int>(Ctx->nb_streams)) {
                    const AVStream *St = Ctx->streams[Q->Track];
                    AVRational R = St->avg_frame_rate.num > 0 && St->avg_frame_rate.den > 0 ? St->avg_frame_rate
                                                                                            : St->r_frame_rate;
                    if (R.num > 0 && R.den > 0) {
                        Q->VP.FPS.Num = R.num;
                        Q->VP.FPS.Den = R.den;
                    }
                }
                if (Q->VP.Duration <= 0 && Ctx->duration > 0)
                    Q->VP.Duration = av_rescale_q(Ctx->duration, AV_TIME_BASE_Q,
                                                  AVRational{Q->VP.TimeBase.Num, Q->VP.TimeBase.Den});
                avformat_close_input(&Ctx);
            }
        }
        /* LWVideoDecoder's estimate ignores the time base (Duration * fps), so redo it. */
        if (Q->VP.Duration > 0)
            Q->VP.NumFrames = av_rescale(Q->VP.Duration, static_cast<int64_t>(Q->VP.TimeBase.Num) * Q->VP.FPS.Num,
                                         static_cast<int64_t>(Q->VP.TimeBase.Den) * Q->VP.FPS.Den);
        if (Q->VP.TimeBase.Num <= 0 || Q->VP.TimeBase.Den <= 0)
            throw BestSourceException("invalid time base");
        AVFrame *F = Q->Dec->GetNextFrame();
        if (!F)
            throw BestSourceException("could not decode the first frame");
        Q->Width = F->width;
        Q->Height = F->height;
        Q->FirstPTS = bs_quick::PtsOf(F);
        if (Q->FirstPTS == AV_NOPTS_VALUE)
            Q->FirstPTS = 0;
        Q->Keep(F, 0);
        *out = Q.release();
        return BS_OK;
    });
}

extern "C" void bs_quick_close(bs_quick *q) {
    delete q;
}

extern "C" void bs_quick_get_props(const bs_quick *q, bs_quick_props *o) {
    if (!q || !o)
        return;
    o->track = q->Track;
    o->width = q->Width;
    o->height = q->Height;
    o->num_frames_estimate = q->VP.NumFrames;
    o->fps_num = q->VP.FPS.Num;
    o->fps_den = q->VP.FPS.Den;
    o->time_base_num = q->VP.TimeBase.Num;
    o->time_base_den = q->VP.TimeBase.Den;
    o->first_pts = q->FirstPTS;
    o->duration = q->VP.Duration > 0 ? q->VP.Duration : 0;
}

extern "C" int bs_quick_get_frame_rgba(bs_quick *q, int64_t n, uint8_t *dst, ptrdiff_t stride, int width, int height,
                                       int64_t *pts_out, char *err, size_t err_len) {
    if (!q || !dst) {
        SetErr(err, err_len, "null argument");
        return BS_ERR;
    }
    return Guard(err, err_len, [&]() -> int {
        q->Get(n < 0 ? 0 : n);
        if (!q->Last)
            throw BestSourceException("no frame");
        if (q->Last->width != width || q->Last->height != height)
            throw BestSourceException("frame size changed");
        q->Conv.Convert(q->Last, width, height, dst, stride);
        if (pts_out)
            *pts_out = bs_quick::PtsOf(q->Last);
        return BS_OK;
    });
}
