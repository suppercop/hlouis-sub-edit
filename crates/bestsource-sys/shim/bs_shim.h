/* Minimal C ABI over BestSource (C++). Part of hlouis-sub-edit, BSD-3-Clause.
 *
 * Every function that can fail returns BS_OK, BS_ERR or BS_CANCELLED and writes a NUL-terminated
 * message into `err` (when non-null) on failure. No C++ exception ever crosses this boundary.
 * Handles are not thread-safe: use one handle from one thread at a time. */
#ifndef HLOUIS_BS_SHIM_H
#define HLOUIS_BS_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define BS_OK 0
#define BS_ERR (-1)
#define BS_CANCELLED (-2)

/* Cache modes, identical to BestSource's BestCacheMode. */
#define BS_CACHE_DISABLE 0
#define BS_CACHE_AUTO_SUBTREE 1
#define BS_CACHE_ALWAYS_SUBTREE 2
#define BS_CACHE_AUTO_ABSOLUTE 3
#define BS_CACHE_ALWAYS_ABSOLUTE 4

/* Indexing progress; return non-zero to cancel. `total` is the file size in bytes. */
typedef int (*bs_progress_cb)(void *user, int track, int64_t current, int64_t total);
/* Polled while seeking/decoding; return non-zero to abandon the request. */
typedef int (*bs_cancel_cb)(void *user);

typedef struct bs_video bs_video;
typedef struct bs_audio bs_audio;

typedef struct bs_video_props {
    int track;
    int width;
    int height;
    int64_t num_frames;
    int fps_num;
    int fps_den;
    int time_base_num;
    int time_base_den;
    int64_t duration;   /* in time base units */
    double start_time;  /* seconds */
    int pix_fmt;        /* AVPixelFormat of the default format set */
    int color_family;   /* 0 unknown, 1 gray, 2 RGB, 3 YUV */
    int bits;
    int sub_sampling_w;
    int sub_sampling_h;
} bs_video_props;

typedef struct bs_audio_props {
    int track;
    int sample_rate;
    int channels;
    int bits_per_sample;
    int bytes_per_sample;
    int is_float;
    int64_t num_samples;
    int64_t num_frames;
    double start_time; /* seconds */
    uint64_t channel_layout;
} bs_audio_props;

int bs_version(void);
void bs_set_ffmpeg_log_level(int level);

/* Tracks in the file: returns the count (>= 0) or BS_ERR. `media_types` (AVMediaType per track)
   is filled up to `cap` entries when non-null. */
int bs_track_list(const char *path_utf8, int *media_types, int cap, char *err, size_t err_len);

/* track < 0 means "the n-th video track" (-1 = first). cache_path may be NULL or "" for the
   default per-user cache. threads <= 0 means automatic. */
int bs_video_open(const char *path_utf8, int track, int threads, int cache_mode,
                  const char *cache_path_utf8, bs_progress_cb progress, void *progress_user,
                  bs_video **out, char *err, size_t err_len);
void bs_video_close(bs_video *v);
void bs_video_set_cancel(bs_video *v, bs_cancel_cb cb, void *user);
void bs_video_get_props(const bs_video *v, bs_video_props *out);
/* Fills `count` frames starting at 0: pts in time base units, keyframe flag 0/1. */
int bs_video_frame_table(const bs_video *v, int64_t *pts, uint8_t *keyframe, int64_t count);
/* Decodes frame n and converts it to tightly or `stride`-packed RGBA8 of width x height. */
int bs_video_get_frame_rgba(bs_video *v, int64_t n, uint8_t *dst, ptrdiff_t stride, int width,
                            int height, int64_t *pts_out, char *err, size_t err_len);
/* Decodes frame n as planar data in its native format (1 byte per sample for <= 8 bits, else 2).
   Plane 0 is luma/G, 1 and 2 chroma/B,R. For gray, planes 1 and 2 are ignored. */
int bs_video_get_frame_planar(bs_video *v, int64_t n, uint8_t *const dst[3],
                              const ptrdiff_t stride[3], char *err, size_t err_len);

int bs_audio_open(const char *path_utf8, int track, int threads, int cache_mode,
                  const char *cache_path_utf8, bs_progress_cb progress, void *progress_user,
                  bs_audio **out, char *err, size_t err_len);
void bs_audio_close(bs_audio *a);
void bs_audio_set_cancel(bs_audio *a, bs_cancel_cb cb, void *user);
void bs_audio_get_props(const bs_audio *a, bs_audio_props *out);
/* Interleaved samples [start, start + count) in the native sample format. Out-of-range parts
   are filled with silence. `dst` must hold count * channels * bytes_per_sample bytes. */
int bs_audio_get_packed(bs_audio *a, uint8_t *dst, int64_t start, int64_t count, char *err,
                        size_t err_len);

#ifdef __cplusplus
}
#endif

#endif
