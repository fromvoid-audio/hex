#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define MAX_VOICES 16u
#define PARAM_GAIN 1u
#define PARAM_RELEASE_FRAMES 2u
#define MAX_TAIL_FRAMES 64u

typedef struct ResVoice {
    uint8_t active;
    uint8_t releasing;
    uint8_t note;
    uint8_t channel;
    float velocity;
    double src_pos;
    uint32_t release_remaining;
    uint32_t release_total;
} ResVoice;

typedef struct ResPlayerState {
    uint32_t sample_rate;
    float gain;
    float release_frames;
    ResVoice voices[MAX_VOICES];
} ResPlayerState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) {
        len = cap;
    }
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t res_describe(HexAbiDescriptor* out) {
    if (!out) {
        return HEX_ABI_INVALID_ARG;
    }
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiDescriptor);
    out->magic = HEX_PACKAGE_ABI_MAGIC;
    out->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out->device_kind = HEX_DEVICE_KIND_INSTRUMENT;
    out->state_version = 1u;
    out->ui_version = 1u;
    out->package_major = 1u;
    out->package_minor = 0u;
    out->package_patch = 0u;
    out->capabilities = HEX_CAP_INSTRUMENT | HEX_CAP_PARAMETER_EVENTS |
                        HEX_CAP_NOTE_EVENTS | HEX_CAP_ZERO_LATENCY |
                        HEX_CAP_STATE_PERSISTENCE | HEX_CAP_BORROWED_RESOURCES;
    out->reported_latency_frames = 0u;
    out->tail_frames = MAX_TAIL_FRAMES;
    out->parameter_count = 2u;
    out->max_state_bytes = (uint32_t)sizeof(ResPlayerState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.resource_player", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t res_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out) {
        return HEX_ABI_INVALID_ARG;
    }
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    if (index == 0u) {
        out->id = PARAM_GAIN;
        out->min = 0.0f;
        out->max = 2.0f;
        out->default_value = 1.0f;
        write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "gain", &out->name_len);
        return HEX_ABI_OK;
    }
    if (index == 1u) {
        out->id = PARAM_RELEASE_FRAMES;
        out->min = 0.0f;
        out->max = (float)MAX_TAIL_FRAMES;
        out->default_value = 32.0f;
        write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "release", &out->name_len);
        return HEX_ABI_OK;
    }
    return HEX_ABI_INVALID_ARG;
}

static int32_t res_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) {
        return HEX_ABI_INVALID_RATE;
    }
    ResPlayerState* st = (ResPlayerState*)calloc(1, sizeof(ResPlayerState));
    if (!st) {
        return HEX_ABI_ALLOCATION_FAILED;
    }
    st->sample_rate = sample_rate;
    st->gain = 1.0f;
    st->release_frames = 32.0f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void res_destroy(void* instance) {
    free(instance);
}

static int32_t res_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(ResPlayerState)) {
        return HEX_ABI_INVALID_STATE;
    }
    ResPlayerState tmp;
    memcpy(&tmp, bytes, sizeof(ResPlayerState));
    if (!isfinite(tmp.gain) || tmp.gain < 0.0f || tmp.gain > 2.0f ||
        !isfinite(tmp.release_frames) || tmp.release_frames < 0.0f ||
        tmp.release_frames > (float)MAX_TAIL_FRAMES) {
        return HEX_ABI_INVALID_STATE;
    }
    ResPlayerState* st = (ResPlayerState*)instance;
    uint32_t sr = st->sample_rate;
    memcpy(st, &tmp, sizeof(ResPlayerState));
    st->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t res_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) {
        return HEX_ABI_INVALID_ARG;
    }
    *out_len = (uint32_t)sizeof(ResPlayerState);
    if (!out_bytes || capacity < sizeof(ResPlayerState)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    memcpy(out_bytes, instance, sizeof(ResPlayerState));
    return HEX_ABI_OK;
}

static void sample_borrowed_resource(
    const HexAbiResourceView* res,
    double src_pos,
    float* out_l,
    float* out_r
) {
    *out_l = 0.0f;
    *out_r = 0.0f;
    if (!res || !res->samples || res->meta.frame_count == 0u || src_pos < 0.0) {
        return;
    }
    uint64_t idx0 = (uint64_t)src_pos;
    if (idx0 >= res->meta.frame_count) {
        return;
    }
    double frac = src_pos - (double)idx0;
    uint64_t idx1 = (idx0 + 1u < res->meta.frame_count) ? (idx0 + 1u) : idx0;
    if (res->meta.channel_layout == HEX_RESOURCE_LAYOUT_MONO) {
        float s0 = res->samples[idx0];
        float s1 = res->samples[idx1];
        float s = (float)((1.0 - frac) * (double)s0 + frac * (double)s1);
        *out_l = s;
        *out_r = s;
    } else if (res->meta.channel_layout == HEX_RESOURCE_LAYOUT_STEREO_INTERLEAVED) {
        float l0 = res->samples[idx0 * 2u];
        float r0 = res->samples[idx0 * 2u + 1u];
        float l1 = res->samples[idx1 * 2u];
        float r1 = res->samples[idx1 * 2u + 1u];
        *out_l = (float)((1.0 - frac) * (double)l0 + frac * (double)l1);
        *out_r = (float)((1.0 - frac) * (double)r0 + frac * (double)r1);
    }
}

static int32_t res_process_core(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_pcm,
    float* out_pcm,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events,
    const HexAbiResourceView* resources,
    uint32_t resource_count
) {
    (void)in_pcm;
    if (!instance || !ctx || !out_pcm) {
        return HEX_ABI_INVALID_ARG;
    }
    if (ctx->struct_size != sizeof(HexAbiProcessContext)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    if (ctx->abi_version != HEX_PACKAGE_ABI_VERSION_V1) {
        return HEX_ABI_UNSUPPORTED_ABI;
    }
    if (ctx->frames == 0u || ctx->frames > HEX_PACKAGE_ABI_MAX_FRAMES) {
        return HEX_ABI_INVALID_FRAMES;
    }
    ResPlayerState* st = (ResPlayerState*)instance;

    /* Locate slot 1 borrowed resource strictly for this call (never cached!). */
    const HexAbiResourceView* slot1 = NULL;
    for (uint32_t i = 0u; i < resource_count; ++i) {
        if (resources && resources[i].meta.slot == 1u) {
            slot1 = &resources[i];
            break;
        }
    }

    uint32_t p_idx = 0u;
    uint32_t n_idx = 0u;
    double step = (slot1 && ctx->sample_rate > 0u)
                      ? ((double)slot1->meta.sample_rate / (double)ctx->sample_rate)
                      : 0.0;

    for (uint32_t f = 0u; f < ctx->frames; ++f) {
        while (p_idx < ctx->param_event_count && param_events && param_events[p_idx].sample_offset == f) {
            uint32_t pid = param_events[p_idx].parameter_id;
            float val = param_events[p_idx].value;
            if (pid == PARAM_GAIN && isfinite(val) && val >= 0.0f && val <= 2.0f) {
                st->gain = val;
            } else if (pid == PARAM_RELEASE_FRAMES && isfinite(val) && val >= 0.0f &&
                       val <= (float)MAX_TAIL_FRAMES) {
                st->release_frames = val;
            }
            p_idx++;
        }
        while (n_idx < ctx->note_event_count && note_events && note_events[n_idx].sample_offset == f) {
            const HexAbiNoteEvent* ev = &note_events[n_idx];
            if (ev->kind == HEX_NOTE_EVENT_ON) {
                for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                    if (!st->voices[v].active) {
                        st->voices[v].active = 1u;
                        st->voices[v].releasing = 0u;
                        st->voices[v].note = ev->note;
                        st->voices[v].channel = ev->channel;
                        st->voices[v].velocity = ev->velocity;
                        st->voices[v].src_pos = 0.0;
                        st->voices[v].release_remaining = 0u;
                        st->voices[v].release_total = 0u;
                        break;
                    }
                }
            } else if (ev->kind == HEX_NOTE_EVENT_OFF) {
                uint32_t rel = (uint32_t)llround((double)st->release_frames);
                if (rel > MAX_TAIL_FRAMES) {
                    rel = MAX_TAIL_FRAMES;
                }
                for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                    if (st->voices[v].active && !st->voices[v].releasing &&
                        st->voices[v].note == ev->note) {
                        if (rel == 0u) {
                            st->voices[v].active = 0u;
                        } else {
                            st->voices[v].releasing = 1u;
                            st->voices[v].release_remaining = rel;
                            st->voices[v].release_total = rel;
                        }
                        break;
                    }
                }
            } else if (ev->kind == HEX_NOTE_EVENT_ALL_NOTES_OFF) {
                for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                    st->voices[v].active = 0u;
                    st->voices[v].releasing = 0u;
                }
            }
            n_idx++;
        }

        double sum_l = 0.0;
        double sum_r = 0.0;
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
            if (!st->voices[v].active) {
                continue;
            }
            float env = 1.0f;
            if (st->voices[v].releasing) {
                if (st->voices[v].release_remaining == 0u) {
                    st->voices[v].active = 0u;
                    continue;
                }
                env = (float)st->voices[v].release_remaining / (float)st->voices[v].release_total;
                st->voices[v].release_remaining--;
                if (st->voices[v].release_remaining == 0u) {
                    /* Render this final tail frame then deactivate on next frame. */
                }
            }
            if (slot1 && slot1->meta.frame_count > 0u) {
                float sl = 0.0f;
                float sr = 0.0f;
                /* Wrap modulo frame_count so sustained/releasing voices keep reading borrowed PCM. */
                double pos = fmod(st->voices[v].src_pos, (double)slot1->meta.frame_count);
                sample_borrowed_resource(slot1, pos, &sl, &sr);
                double scale = (double)st->voices[v].velocity * (double)st->gain * (double)env;
                sum_l += (double)sl * scale;
                sum_r += (double)sr * scale;
                st->voices[v].src_pos += step;
            }
        }
        out_pcm[f * 2u] = (float)sum_l;
        out_pcm[f * 2u + 1u] = (float)sum_r;
    }
    return HEX_ABI_OK;
}

static int32_t res_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_pcm,
    float* out_pcm,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    return res_process_core(instance, ctx, in_pcm, out_pcm, param_events, note_events, NULL, 0u);
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) {
        return HEX_ABI_INVALID_ARG;
    }
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) {
        return HEX_ABI_UNSUPPORTED_ABI;
    }
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = res_describe;
    out_vtable->parameter_descriptor = res_param_desc;
    out_vtable->create = res_create;
    out_vtable->destroy = res_destroy;
    out_vtable->set_state = res_set_state;
    out_vtable->get_state = res_get_state;
    out_vtable->process = res_process;
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_resources_entry(
    uint32_t requested_abi_version,
    HexAbiResourceVtableV1* out_vtable
) {
    if (!out_vtable) {
        return HEX_ABI_INVALID_ARG;
    }
    if (out_vtable->struct_size != sizeof(HexAbiResourceVtableV1)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) {
        return HEX_ABI_UNSUPPORTED_ABI;
    }
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->process_with_resources = res_process_core;
    return HEX_ABI_OK;
}
