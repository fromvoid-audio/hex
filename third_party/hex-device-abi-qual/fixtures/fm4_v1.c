#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define MAX_VOICES 16u
#define TWO_PI 6.283185307179586476925286766559

#define PARAM_GAIN 1u
#define PARAM_MOD_INDEX 2u
#define PARAM_RATIO 3u
#define PARAM_FEEDBACK 4u
#define PARAM_ATTACK_MS 5u
#define PARAM_DECAY_MS 6u
#define PARAM_SUSTAIN 7u
#define PARAM_RELEASE_MS 8u

typedef struct Fm4Voice {
    uint8_t active;
    uint8_t held;
    uint8_t note;
    uint8_t reserved;
    float velocity;
    float base_detune_cents;
    float note_bend_cents;
    float note_pressure;
    float env;
    uint8_t env_stage;
    double carrier_phase;
    double mod_phase;
    float fb_prev;
} Fm4Voice;

typedef struct Fm4State {
    uint32_t sample_rate;
    float gain;
    float mod_index;
    float ratio;
    float feedback;
    float attack_ms;
    float decay_ms;
    float sustain;
    float release_ms;
    float channel_bend_cents;
    float channel_pressure;
    Fm4Voice voices[MAX_VOICES];
} Fm4State;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) len = cap;
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static double midi_to_hz(uint8_t note, float total_cents) {
    double semitones = ((double)note - 69.0) + ((double)total_cents / 100.0);
    return 440.0 * pow(2.0, semitones / 12.0);
}

static int32_t fm4_describe(HexAbiDescriptor* out) {
    if (!out) return HEX_ABI_INVALID_ARG;
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
    out->tail_frames = 48000u;
    out->parameter_count = 8u;
    out->max_state_bytes = (uint32_t)sizeof(Fm4State);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.fm4", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t fm4_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 8u) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    out->id = index + 1u;
    switch (out->id) {
        case PARAM_GAIN: out->min = 0.0f; out->max = 2.0f; out->default_value = 0.5f; write_str(out->name, 64, "gain", &out->name_len); break;
        case PARAM_MOD_INDEX: out->min = 0.0f; out->max = 8.0f; out->default_value = 2.0f; write_str(out->name, 64, "depth", &out->name_len); break;
        case PARAM_RATIO: out->min = 0.5f; out->max = 16.0f; out->default_value = 2.0f; write_str(out->name, 64, "ratio", &out->name_len); break;
        case PARAM_FEEDBACK: out->min = 0.0f; out->max = 1.0f; out->default_value = 0.2f; write_str(out->name, 64, "feedback", &out->name_len); break;
        case PARAM_ATTACK_MS: out->min = 0.5f; out->max = 2000.0f; out->default_value = 4.0f; write_str(out->name, 64, "attack", &out->name_len); break;
        case PARAM_DECAY_MS: out->min = 1.0f; out->max = 4000.0f; out->default_value = 120.0f; write_str(out->name, 64, "decay", &out->name_len); break;
        case PARAM_SUSTAIN: out->min = 0.0f; out->max = 1.0f; out->default_value = 0.6f; write_str(out->name, 64, "mix", &out->name_len); break;
        case PARAM_RELEASE_MS: out->min = 1.0f; out->max = 2000.0f; out->default_value = 100.0f; write_str(out->name, 64, "release", &out->name_len); break;
        default: return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t fm4_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) return HEX_ABI_INVALID_RATE;
    Fm4State* st = (Fm4State*)calloc(1, sizeof(Fm4State));
    if (!st) return HEX_ABI_ALLOCATION_FAILED;
    st->sample_rate = sample_rate;
    st->gain = 0.5f;
    st->mod_index = 2.0f;
    st->ratio = 2.0f;
    st->feedback = 0.2f;
    st->attack_ms = 4.0f;
    st->decay_ms = 120.0f;
    st->sustain = 0.6f;
    st->release_ms = 100.0f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void fm4_destroy(void* instance) { free(instance); }

static int32_t fm4_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(Fm4State)) return HEX_ABI_INVALID_STATE;
    Fm4State cand;
    memcpy(&cand, bytes, sizeof(Fm4State));
    if (!isfinite(cand.gain) || cand.gain < 0.0f || cand.gain > 2.0f ||
        !isfinite(cand.mod_index) || cand.mod_index < 0.0f || cand.mod_index > 8.0f ||
        !isfinite(cand.ratio) || cand.ratio < 0.5f || cand.ratio > 16.0f) {
        return HEX_ABI_INVALID_STATE;
    }
    uint32_t sr = ((Fm4State*)instance)->sample_rate;
    memcpy(instance, &cand, sizeof(Fm4State));
    ((Fm4State*)instance)->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t fm4_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) return HEX_ABI_INVALID_ARG;
    *out_len = (uint32_t)sizeof(Fm4State);
    if (!out_bytes || capacity < sizeof(Fm4State)) return HEX_ABI_SIZE_MISMATCH;
    memcpy(out_bytes, instance, sizeof(Fm4State));
    return HEX_ABI_OK;
}

static void handle_event(Fm4State* st, const HexAbiNoteEvent* ev) {
    if (ev->kind == HEX_NOTE_EVENT_ON) {
        uint32_t slot = MAX_VOICES;
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
            if (!st->voices[v].active) { slot = v; break; }
        }
        if (slot == MAX_VOICES) slot = 0u;
        Fm4Voice* vc = &st->voices[slot];
        memset(vc, 0, sizeof(*vc));
        vc->active = 1u;
        vc->held = 1u;
        vc->note = ev->note;
        vc->velocity = isfinite(ev->velocity) ? ev->velocity : 0.0f;
        vc->base_detune_cents = isfinite(ev->detune_cents) ? ev->detune_cents : 0.0f;
        vc->env_stage = 1u;
    } else if (ev->kind == HEX_NOTE_EVENT_OFF) {
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
            if (st->voices[v].active && st->voices[v].held && st->voices[v].note == ev->note) {
                st->voices[v].held = 0u;
                st->voices[v].env_stage = 4u;
            }
        }
    } else if (ev->kind == HEX_NOTE_EVENT_ALL_NOTES_OFF) {
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) st->voices[v].active = 0u;
        st->channel_bend_cents = 0.0f;
        st->channel_pressure = 0.0f;
    } else if (ev->kind == HEX_NOTE_EVENT_PITCH_BEND && isfinite(ev->detune_cents)) {
        if (ev->note == 0xFFu) st->channel_bend_cents = ev->detune_cents;
        else {
            for (uint32_t v = 0u; v < MAX_VOICES; ++v)
                if (st->voices[v].active && st->voices[v].note == ev->note)
                    st->voices[v].note_bend_cents = ev->detune_cents;
        }
    } else if (ev->kind == HEX_NOTE_EVENT_PRESSURE && isfinite(ev->velocity)) {
        if (ev->note == 0xFFu) st->channel_pressure = ev->velocity;
        else {
            for (uint32_t v = 0u; v < MAX_VOICES; ++v)
                if (st->voices[v].active && st->voices[v].note == ev->note)
                    st->voices[v].note_pressure = ev->velocity;
        }
    }
}

static int32_t fm4_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_stereo,
    float* out_stereo,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    (void)in_stereo;
    if (!instance || !ctx || !out_stereo) return HEX_ABI_INVALID_ARG;
    if (ctx->struct_size != sizeof(HexAbiProcessContext)) return HEX_ABI_SIZE_MISMATCH;
    if (ctx->abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (ctx->frames == 0u || ctx->frames > HEX_PACKAGE_ABI_MAX_FRAMES) return HEX_ABI_INVALID_FRAMES;

    Fm4State* st = (Fm4State*)instance;
    double sr = (double)st->sample_rate;
    uint32_t p_idx = 0u, n_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && p_idx < ctx->param_event_count && param_events[p_idx].sample_offset <= i) {
            uint32_t id = param_events[p_idx].parameter_id;
            float v = param_events[p_idx].value;
            if (isfinite(v)) {
                if (id == PARAM_GAIN && v >= 0.0f && v <= 2.0f) st->gain = v;
                else if (id == PARAM_MOD_INDEX && v >= 0.0f && v <= 8.0f) st->mod_index = v;
                else if (id == PARAM_RATIO && v >= 0.5f && v <= 16.0f) st->ratio = v;
                else if (id == PARAM_FEEDBACK && v >= 0.0f && v <= 1.0f) st->feedback = v;
                else if (id == PARAM_ATTACK_MS && v >= 0.5f && v <= 2000.0f) st->attack_ms = v;
                else if (id == PARAM_DECAY_MS && v >= 1.0f && v <= 4000.0f) st->decay_ms = v;
                else if (id == PARAM_SUSTAIN && v >= 0.0f && v <= 1.0f) st->sustain = v;
                else if (id == PARAM_RELEASE_MS && v >= 1.0f && v <= 2000.0f) st->release_ms = v;
            }
            ++p_idx;
        }
        while (note_events && n_idx < ctx->note_event_count && note_events[n_idx].sample_offset <= i) {
            handle_event(st, &note_events[n_idx]);
            ++n_idx;
        }
        float atk_step = 1.0f / fmaxf(1.0f, st->attack_ms * 0.001f * (float)sr);
        float dec_step = 1.0f / fmaxf(1.0f, st->decay_ms * 0.001f * (float)sr);
        float rel_step = 1.0f / fmaxf(1.0f, st->release_ms * 0.001f * (float)sr);
        double sum = 0.0;
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
            Fm4Voice* vc = &st->voices[v];
            if (!vc->active) continue;
            if (vc->env_stage == 1u) {
                vc->env += atk_step;
                if (vc->env >= 1.0f) { vc->env = 1.0f; vc->env_stage = 2u; }
            } else if (vc->env_stage == 2u) {
                vc->env -= dec_step * (1.0f - st->sustain);
                if (vc->env <= st->sustain) { vc->env = st->sustain; vc->env_stage = 3u; }
            } else if (vc->env_stage == 4u) {
                vc->env -= rel_step;
                if (vc->env <= 0.0001f) { vc->env = 0.0f; vc->active = 0u; continue; }
            }
            float cents = vc->base_detune_cents + vc->note_bend_cents + st->channel_bend_cents;
            double hz = midi_to_hz(vc->note, cents);
            double modulator = sin(vc->mod_phase + (double)(st->feedback * vc->fb_prev));
            vc->fb_prev = (float)modulator;
            double carrier = sin(vc->carrier_phase + modulator * (double)st->mod_index * (double)vc->env);
            vc->carrier_phase += (TWO_PI * hz) / sr;
            if (vc->carrier_phase >= TWO_PI) vc->carrier_phase -= TWO_PI;
            vc->mod_phase += (TWO_PI * hz * (double)st->ratio) / sr;
            if (vc->mod_phase >= TWO_PI) vc->mod_phase -= TWO_PI;
            float expr = 1.0f + 0.4f * (vc->note_pressure + st->channel_pressure);
            sum += carrier * (double)(vc->velocity * vc->env * expr);
        }
        float sample = (float)(sum * (double)st->gain);
        out_stereo[2u * i] = sample;
        out_stereo[2u * i + 1u] = sample;
    }
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) return HEX_ABI_INVALID_ARG;
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) return HEX_ABI_SIZE_MISMATCH;
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = fm4_describe;
    out_vtable->parameter_descriptor = fm4_param_desc;
    out_vtable->create = fm4_create;
    out_vtable->destroy = fm4_destroy;
    out_vtable->set_state = fm4_set_state;
    out_vtable->get_state = fm4_get_state;
    out_vtable->process = fm4_process;
    return HEX_ABI_OK;
}
