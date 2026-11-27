#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define MAX_VOICES 16u
#define MAX_TAIL_FRAMES 48000u

#define PARAM_GAIN 1u
#define PARAM_START 2u
#define PARAM_END 3u
#define PARAM_MODE 4u
#define PARAM_LOOP_ENABLE 5u
#define PARAM_TRANSPOSE 6u
#define PARAM_ATTACK 7u
#define PARAM_DECAY 8u
#define PARAM_SUSTAIN 9u
#define PARAM_RELEASE 10u

enum {
    ENV_OFF = 0,
    ENV_ATTACK = 1,
    ENV_DECAY = 2,
    ENV_SUSTAIN = 3,
    ENV_RELEASE = 4
};

typedef struct SamplerVoice {
    uint8_t active;
    uint8_t env_stage;
    uint8_t note;
    uint8_t channel;
    float velocity;
    float env_level;
    double src_pos;
} SamplerVoice;

typedef struct SamplerPkgState {
    uint32_t sample_rate;
    float gain;
    float start_norm;
    float end_norm;
    float mode;
    float loop_enable;
    float transpose;
    float attack;
    float decay;
    float sustain;
    float release;
    SamplerVoice voices[MAX_VOICES];
} SamplerPkgState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) {
        len = cap;
    }
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t sampler_describe(HexAbiDescriptor* out) {
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
    out->parameter_count = 10u;
    out->max_state_bytes = (uint32_t)sizeof(SamplerPkgState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.sampler", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t sampler_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out) {
        return HEX_ABI_INVALID_ARG;
    }
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    switch (index) {
        case 0u:
            out->id = PARAM_GAIN;
            out->min = 0.0f;
            out->max = 2.0f;
            out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "gain", &out->name_len);
            return HEX_ABI_OK;
        case 1u:
            out->id = PARAM_START;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "start", &out->name_len);
            return HEX_ABI_OK;
        case 2u:
            out->id = PARAM_END;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "end", &out->name_len);
            return HEX_ABI_OK;
        case 3u:
            out->id = PARAM_MODE;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "mode", &out->name_len);
            return HEX_ABI_OK;
        case 4u:
            out->id = PARAM_LOOP_ENABLE;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "loop", &out->name_len);
            return HEX_ABI_OK;
        case 5u:
            out->id = PARAM_TRANSPOSE;
            out->min = -24.0f;
            out->max = 24.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "transpose", &out->name_len);
            return HEX_ABI_OK;
        case 6u:
            out->id = PARAM_ATTACK;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "attack", &out->name_len);
            return HEX_ABI_OK;
        case 7u:
            out->id = PARAM_DECAY;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "decay", &out->name_len);
            return HEX_ABI_OK;
        case 8u:
            out->id = PARAM_SUSTAIN;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 1.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "sustain", &out->name_len);
            return HEX_ABI_OK;
        case 9u:
            out->id = PARAM_RELEASE;
            out->min = 0.0f;
            out->max = 1.0f;
            out->default_value = 0.05f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "release", &out->name_len);
            return HEX_ABI_OK;
        default:
            return HEX_ABI_INVALID_ARG;
    }
}

static int32_t sampler_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) {
        return HEX_ABI_INVALID_RATE;
    }
    SamplerPkgState* st = (SamplerPkgState*)calloc(1, sizeof(SamplerPkgState));
    if (!st) {
        return HEX_ABI_ALLOCATION_FAILED;
    }
    st->sample_rate = sample_rate;
    st->gain = 1.0f;
    st->start_norm = 0.0f;
    st->end_norm = 1.0f;
    st->mode = 0.0f;
    st->loop_enable = 0.0f;
    st->transpose = 0.0f;
    st->attack = 0.0f;
    st->decay = 0.0f;
    st->sustain = 1.0f;
    st->release = 0.05f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void sampler_destroy(void* instance) {
    free(instance);
}

static int32_t sampler_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(SamplerPkgState)) {
        return HEX_ABI_INVALID_STATE;
    }
    SamplerPkgState tmp;
    memcpy(&tmp, bytes, sizeof(SamplerPkgState));
    if (!isfinite(tmp.gain) || tmp.gain < 0.0f || tmp.gain > 2.0f ||
        !isfinite(tmp.start_norm) || tmp.start_norm < 0.0f || tmp.start_norm > 1.0f ||
        !isfinite(tmp.end_norm) || tmp.end_norm < 0.0f || tmp.end_norm > 1.0f ||
        !isfinite(tmp.mode) || tmp.mode < 0.0f || tmp.mode > 1.0f ||
        !isfinite(tmp.loop_enable) || tmp.loop_enable < 0.0f || tmp.loop_enable > 1.0f ||
        !isfinite(tmp.transpose) || tmp.transpose < -24.0f || tmp.transpose > 24.0f ||
        !isfinite(tmp.attack) || tmp.attack < 0.0f || tmp.attack > 1.0f ||
        !isfinite(tmp.decay) || tmp.decay < 0.0f || tmp.decay > 1.0f ||
        !isfinite(tmp.sustain) || tmp.sustain < 0.0f || tmp.sustain > 1.0f ||
        !isfinite(tmp.release) || tmp.release < 0.0f || tmp.release > 1.0f) {
        return HEX_ABI_INVALID_STATE;
    }
    SamplerPkgState* st = (SamplerPkgState*)instance;
    uint32_t sr = st->sample_rate;
    memcpy(st, &tmp, sizeof(SamplerPkgState));
    st->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t sampler_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) {
        return HEX_ABI_INVALID_ARG;
    }
    *out_len = (uint32_t)sizeof(SamplerPkgState);
    if (!out_bytes || capacity < sizeof(SamplerPkgState)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    memcpy(out_bytes, instance, sizeof(SamplerPkgState));
    return HEX_ABI_OK;
}

static void read_sample_linear(
    const HexAbiResourceView* res,
    double pos,
    float* out_l,
    float* out_r
) {
    *out_l = 0.0f;
    *out_r = 0.0f;
    if (!res || !res->samples || res->meta.frame_count == 0u || pos < 0.0) {
        return;
    }
    uint64_t idx0 = (uint64_t)pos;
    if (idx0 >= res->meta.frame_count) {
        return;
    }
    double frac = pos - (double)idx0;
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

static void advance_adsr(SamplerVoice* v, const SamplerPkgState* st) {
    double sr = (double)st->sample_rate;
    if (v->env_stage == ENV_ATTACK) {
        if (st->attack <= 1e-6f) {
            v->env_level = 1.0f;
            v->env_stage = (st->decay <= 1e-6f) ? ENV_SUSTAIN : ENV_DECAY;
            if (v->env_stage == ENV_SUSTAIN) {
                v->env_level = st->sustain;
            }
        } else {
            v->env_level += (float)(1.0 / (st->attack * sr));
            if (v->env_level >= 1.0f) {
                v->env_level = 1.0f;
                v->env_stage = ENV_DECAY;
            }
        }
    } else if (v->env_stage == ENV_DECAY) {
        if (st->decay <= 1e-6f) {
            v->env_level = st->sustain;
            v->env_stage = ENV_SUSTAIN;
        } else {
            v->env_level -= (float)((1.0 - (double)st->sustain) / (st->decay * sr));
            if (v->env_level <= st->sustain) {
                v->env_level = st->sustain;
                v->env_stage = ENV_SUSTAIN;
            }
        }
    } else if (v->env_stage == ENV_SUSTAIN) {
        v->env_level = st->sustain;
    } else if (v->env_stage == ENV_RELEASE) {
        if (st->release <= 1e-6f) {
            v->env_level = 0.0f;
            v->env_stage = ENV_OFF;
            v->active = 0u;
        } else {
            v->env_level -= (float)(1.0 / (st->release * sr));
            if (v->env_level <= 0.0f) {
                v->env_level = 0.0f;
                v->env_stage = ENV_OFF;
                v->active = 0u;
            }
        }
    }
}

static int32_t sampler_process_core(
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
    SamplerPkgState* st = (SamplerPkgState*)instance;

    /* Strictly call-scoped lookup of slot 1 borrowed PCM (empty sampler => NULL => silence). */
    const HexAbiResourceView* slot1 = NULL;
    for (uint32_t i = 0u; i < resource_count; ++i) {
        if (resources && resources[i].meta.slot == 1u) {
            slot1 = &resources[i];
            break;
        }
    }

    uint32_t p_idx = 0u;
    uint32_t n_idx = 0u;

    for (uint32_t f = 0u; f < ctx->frames; ++f) {
        while (p_idx < ctx->param_event_count && param_events && param_events[p_idx].sample_offset == f) {
            uint32_t pid = param_events[p_idx].parameter_id;
            float val = param_events[p_idx].value;
            if (isfinite(val)) {
                if (pid == PARAM_GAIN && val >= 0.0f && val <= 2.0f) st->gain = val;
                else if (pid == PARAM_START && val >= 0.0f && val <= 1.0f) st->start_norm = val;
                else if (pid == PARAM_END && val >= 0.0f && val <= 1.0f) st->end_norm = val;
                else if (pid == PARAM_MODE && val >= 0.0f && val <= 1.0f) st->mode = val;
                else if (pid == PARAM_LOOP_ENABLE && val >= 0.0f && val <= 1.0f) st->loop_enable = val;
                else if (pid == PARAM_TRANSPOSE && val >= -24.0f && val <= 24.0f) st->transpose = val;
                else if (pid == PARAM_ATTACK && val >= 0.0f && val <= 1.0f) st->attack = val;
                else if (pid == PARAM_DECAY && val >= 0.0f && val <= 1.0f) st->decay = val;
                else if (pid == PARAM_SUSTAIN && val >= 0.0f && val <= 1.0f) st->sustain = val;
                else if (pid == PARAM_RELEASE && val >= 0.0f && val <= 1.0f) st->release = val;
            }
            p_idx++;
        }

        double region_start = 0.0;
        double region_end = 0.0;
        if (slot1 && slot1->meta.frame_count > 0u) {
            double total = (double)slot1->meta.frame_count;
            region_start = (double)st->start_norm * total;
            region_end = (double)st->end_norm * total;
            if (region_end <= region_start) {
                region_end = total;
            }
        }

        while (n_idx < ctx->note_event_count && note_events && note_events[n_idx].sample_offset == f) {
            const HexAbiNoteEvent* ev = &note_events[n_idx];
            if (ev->kind == HEX_NOTE_EVENT_ON) {
                for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                    if (!st->voices[v].active) {
                        st->voices[v].active = 1u;
                        st->voices[v].note = ev->note;
                        st->voices[v].channel = ev->channel;
                        st->voices[v].velocity = ev->velocity;
                        st->voices[v].src_pos = region_start;
                        st->voices[v].env_stage = ENV_ATTACK;
                        st->voices[v].env_level = (st->attack <= 1e-6f) ? 1.0f : 0.0f;
                        break;
                    }
                }
            } else if (ev->kind == HEX_NOTE_EVENT_OFF) {
                /* One-shot trigger mode (mode >= 0.5) ignores NoteOff and plays to region_end. */
                if (st->mode < 0.5f) {
                    for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                        if (st->voices[v].active && st->voices[v].env_stage != ENV_RELEASE &&
                            st->voices[v].note == ev->note) {
                            st->voices[v].env_stage = ENV_RELEASE;
                            break;
                        }
                    }
                }
            } else if (ev->kind == HEX_NOTE_EVENT_ALL_NOTES_OFF) {
                for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                    st->voices[v].active = 0u;
                    st->voices[v].env_stage = ENV_OFF;
                    st->voices[v].env_level = 0.0f;
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
            advance_adsr(&st->voices[v], st);
            if (!st->voices[v].active) {
                continue;
            }
            if (!slot1 || slot1->meta.frame_count == 0u || region_end <= region_start) {
                continue;
            }
            if (st->voices[v].src_pos >= region_end) {
                if (st->loop_enable >= 0.5f) {
                    double span = region_end - region_start;
                    st->voices[v].src_pos = region_start + fmod(st->voices[v].src_pos - region_start, span);
                } else {
                    st->voices[v].active = 0u;
                    st->voices[v].env_stage = ENV_OFF;
                    continue;
                }
            }
            float sl = 0.0f;
            float sr = 0.0f;
            read_sample_linear(slot1, st->voices[v].src_pos, &sl, &sr);
            double scale = (double)st->voices[v].velocity * (double)st->voices[v].env_level * (double)st->gain;
            sum_l += (double)sl * scale;
            sum_r += (double)sr * scale;

            double semitones = ((double)st->voices[v].note - 60.0) + (double)st->transpose;
            double pitch_ratio = pow(2.0, semitones / 12.0);
            double step = ((double)slot1->meta.sample_rate / (double)ctx->sample_rate) * pitch_ratio;
            st->voices[v].src_pos += step;
        }
        out_pcm[f * 2u] = (float)sum_l;
        out_pcm[f * 2u + 1u] = (float)sum_r;
    }
    return HEX_ABI_OK;
}

static int32_t sampler_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_pcm,
    float* out_pcm,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    return sampler_process_core(instance, ctx, in_pcm, out_pcm, param_events, note_events, NULL, 0u);
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
    out_vtable->describe = sampler_describe;
    out_vtable->parameter_descriptor = sampler_param_desc;
    out_vtable->create = sampler_create;
    out_vtable->destroy = sampler_destroy;
    out_vtable->set_state = sampler_set_state;
    out_vtable->get_state = sampler_get_state;
    out_vtable->process = sampler_process;
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
    out_vtable->process_with_resources = sampler_process_core;
    return HEX_ABI_OK;
}
