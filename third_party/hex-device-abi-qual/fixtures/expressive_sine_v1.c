#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define MAX_VOICES 16u
#define PARAM_GAIN 1u
#define TWO_PI 6.283185307179586476925286766559

typedef struct ExpressiveVoice {
    uint8_t active;
    uint8_t note;
    uint8_t channel;
    uint8_t reserved;
    float velocity;
    float base_detune_cents;
    float note_bend_cents;
    float note_pressure;
    double phase;
} ExpressiveVoice;

typedef struct ExpressiveSineState {
    uint32_t sample_rate;
    float gain;
    float channel_bend_cents;
    float channel_pressure;
    float cc_mod_depth;
    ExpressiveVoice voices[MAX_VOICES];
} ExpressiveSineState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) {
        len = cap;
    }
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static double midi_to_hz(uint8_t note, float total_detune_cents) {
    double semitones = ((double)note - 69.0) + ((double)total_detune_cents / 100.0);
    return 440.0 * pow(2.0, semitones / 12.0);
}

static int32_t exp_describe(HexAbiDescriptor* out) {
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
                        HEX_CAP_STATE_PERSISTENCE | HEX_CAP_NOTE_EXPRESSION |
                        HEX_CAP_CHANNEL_EXPRESSION;
    out->reported_latency_frames = 0u;
    out->tail_frames = 0u;
    out->parameter_count = 1u;
    out->max_state_bytes = (uint32_t)sizeof(float);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.expressive_sine", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t exp_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index != 0u) {
        return HEX_ABI_INVALID_ARG;
    }
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    out->id = PARAM_GAIN;
    out->min = 0.0f;
    out->max = 2.0f;
    out->default_value = 0.5f;
    write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "gain", &out->name_len);
    return HEX_ABI_OK;
}

static int32_t exp_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) {
        return HEX_ABI_INVALID_RATE;
    }
    ExpressiveSineState* st = (ExpressiveSineState*)calloc(1, sizeof(ExpressiveSineState));
    if (!st) {
        return HEX_ABI_ALLOCATION_FAILED;
    }
    st->sample_rate = sample_rate;
    st->gain = 0.5f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void exp_destroy(void* instance) {
    free(instance);
}

static int32_t exp_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(float)) {
        return HEX_ABI_INVALID_STATE;
    }
    float gain = 0.0f;
    memcpy(&gain, bytes, sizeof(float));
    if (!isfinite(gain) || gain < 0.0f || gain > 2.0f) {
        return HEX_ABI_INVALID_STATE;
    }
    ((ExpressiveSineState*)instance)->gain = gain;
    return HEX_ABI_OK;
}

static int32_t exp_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) {
        return HEX_ABI_INVALID_ARG;
    }
    *out_len = (uint32_t)sizeof(float);
    if (!out_bytes || capacity < sizeof(float)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    float gain = ((const ExpressiveSineState*)instance)->gain;
    memcpy(out_bytes, &gain, sizeof(float));
    return HEX_ABI_OK;
}

static void handle_event(ExpressiveSineState* st, const HexAbiNoteEvent* ev) {
    switch (ev->kind) {
        case HEX_NOTE_EVENT_ON: {
            uint32_t slot = MAX_VOICES;
            for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                if (!st->voices[v].active) {
                    slot = v;
                    break;
                }
            }
            if (slot == MAX_VOICES) {
                slot = 0u;
            }
            st->voices[slot].active = 1u;
            st->voices[slot].note = ev->note;
            st->voices[slot].channel = ev->channel;
            st->voices[slot].velocity = isfinite(ev->velocity) ? ev->velocity : 0.0f;
            st->voices[slot].base_detune_cents = isfinite(ev->detune_cents) ? ev->detune_cents : 0.0f;
            st->voices[slot].note_bend_cents = 0.0f;
            st->voices[slot].note_pressure = 0.0f;
            st->voices[slot].phase = 0.0;
            break;
        }
        case HEX_NOTE_EVENT_OFF:
            for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                if (st->voices[v].active && st->voices[v].note == ev->note) {
                    st->voices[v].active = 0u;
                }
            }
            break;
        case HEX_NOTE_EVENT_ALL_NOTES_OFF:
            for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                st->voices[v].active = 0u;
            }
            st->channel_bend_cents = 0.0f;
            st->channel_pressure = 0.0f;
            st->cc_mod_depth = 0.0f;
            break;
        case HEX_NOTE_EVENT_PITCH_BEND:
            if (isfinite(ev->detune_cents)) {
                if (ev->note == 0xFFu) {
                    st->channel_bend_cents = ev->detune_cents;
                } else {
                    for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                        if (st->voices[v].active && st->voices[v].note == ev->note) {
                            st->voices[v].note_bend_cents = ev->detune_cents;
                        }
                    }
                }
            }
            break;
        case HEX_NOTE_EVENT_PRESSURE:
            if (isfinite(ev->velocity)) {
                if (ev->note == 0xFFu) {
                    st->channel_pressure = ev->velocity;
                } else {
                    for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
                        if (st->voices[v].active && st->voices[v].note == ev->note) {
                            st->voices[v].note_pressure = ev->velocity;
                        }
                    }
                }
            }
            break;
        case HEX_NOTE_EVENT_CC:
            if (ev->note == 1u && isfinite(ev->velocity)) {
                st->cc_mod_depth = ev->velocity;
            }
            break;
        default:
            break;
    }
}

static int32_t exp_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_stereo,
    float* out_stereo,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    (void)in_stereo;
    if (!instance || !ctx || !out_stereo) {
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
    ExpressiveSineState* st = (ExpressiveSineState*)instance;
    uint32_t p_idx = 0u;
    uint32_t n_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && p_idx < ctx->param_event_count && param_events[p_idx].sample_offset <= i) {
            if (param_events[p_idx].parameter_id == PARAM_GAIN &&
                isfinite(param_events[p_idx].value) &&
                param_events[p_idx].value >= 0.0f &&
                param_events[p_idx].value <= 2.0f) {
                st->gain = param_events[p_idx].value;
            }
            ++p_idx;
        }
        while (note_events && n_idx < ctx->note_event_count && note_events[n_idx].sample_offset <= i) {
            handle_event(st, &note_events[n_idx]);
            ++n_idx;
        }
        double sum = 0.0;
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
            if (st->voices[v].active) {
                float total_cents = st->voices[v].base_detune_cents +
                                    st->voices[v].note_bend_cents +
                                    st->channel_bend_cents;
                double hz = midi_to_hz(st->voices[v].note, total_cents);
                double inc = (TWO_PI * hz) / (double)st->sample_rate;
                float pressure_boost = 1.0f + 0.5f * (st->voices[v].note_pressure + st->channel_pressure) + 0.25f * st->cc_mod_depth;
                sum += sin(st->voices[v].phase) * (double)(st->voices[v].velocity * pressure_boost);
                st->voices[v].phase += inc;
                if (st->voices[v].phase >= TWO_PI) {
                    st->voices[v].phase -= TWO_PI;
                }
            }
        }
        float sample = (float)(sum * (double)st->gain);
        out_stereo[2u * i] = sample;
        out_stereo[2u * i + 1u] = sample;
    }
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) {
        return HEX_ABI_INVALID_ARG;
    }
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) {
        return HEX_ABI_UNSUPPORTED_ABI;
    }
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) {
        return HEX_ABI_SIZE_MISMATCH;
    }
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = exp_describe;
    out_vtable->parameter_descriptor = exp_param_desc;
    out_vtable->create = exp_create;
    out_vtable->destroy = exp_destroy;
    out_vtable->set_state = exp_set_state;
    out_vtable->get_state = exp_get_state;
    out_vtable->process = exp_process;
    return HEX_ABI_OK;
}
