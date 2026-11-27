#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define MAX_VOICES 16u
#define TWO_PI 6.283185307179586476925286766559

#define PARAM_GAIN 1u
#define PARAM_ALGORITHM 2u
#define PARAM_OP1_RATIO 3u
#define PARAM_OP1_LEVEL 4u
#define PARAM_OP2_RATIO 5u
#define PARAM_OP2_LEVEL 6u
#define PARAM_OP3_RATIO 7u
#define PARAM_OP3_LEVEL 8u
#define PARAM_OP4_RATIO 9u
#define PARAM_OP4_LEVEL 10u
#define PARAM_FEEDBACK 11u
#define PARAM_ATTACK_MS 12u
#define PARAM_DECAY_MS 13u
#define PARAM_SUSTAIN 14u
#define PARAM_RELEASE_MS 15u
#define PARAM_CUTOFF_HZ 16u

typedef struct OpVoice {
    uint8_t active;
    uint8_t held;
    uint8_t note;
    uint8_t reserved;
    float velocity;
    float base_detune_cents;
    float note_bend_cents;
    float note_pressure;
    float env;
    uint8_t env_stage; /* 0=idle, 1=attack, 2=decay, 3=sustain, 4=release */
    double phase[4];
    float fb_prev;
    float lpf_z1;
} OpVoice;

typedef struct OperatorState {
    uint32_t sample_rate;
    float gain;
    float algorithm;
    float ratio[4];
    float level[4];
    float feedback;
    float attack_ms;
    float decay_ms;
    float sustain;
    float release_ms;
    float cutoff_hz;
    float channel_bend_cents;
    float channel_pressure;
    OpVoice voices[MAX_VOICES];
} OperatorState;

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

static int32_t op_describe(HexAbiDescriptor* out) {
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
    out->parameter_count = 16u;
    out->max_state_bytes = (uint32_t)sizeof(OperatorState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.operator", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t op_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 16u) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    out->id = index + 1u;
    switch (out->id) {
        case PARAM_GAIN: out->min = 0.0f; out->max = 2.0f; out->default_value = 0.5f; write_str(out->name, 64, "gain", &out->name_len); break;
        case PARAM_ALGORITHM: out->min = 0.0f; out->max = 2.0f; out->default_value = 0.0f; write_str(out->name, 64, "amount", &out->name_len); break;
        case PARAM_OP1_RATIO: out->min = 0.5f; out->max = 16.0f; out->default_value = 1.0f; write_str(out->name, 64, "rate", &out->name_len); break;
        case PARAM_OP1_LEVEL: out->min = 0.0f; out->max = 1.0f; out->default_value = 1.0f; write_str(out->name, 64, "depth", &out->name_len); break;
        case PARAM_OP2_RATIO: out->min = 0.5f; out->max = 16.0f; out->default_value = 2.0f; write_str(out->name, 64, "rate", &out->name_len); break;
        case PARAM_OP2_LEVEL: out->min = 0.0f; out->max = 1.0f; out->default_value = 0.4f; write_str(out->name, 64, "depth", &out->name_len); break;
        case PARAM_OP3_RATIO: out->min = 0.5f; out->max = 16.0f; out->default_value = 3.0f; write_str(out->name, 64, "rate", &out->name_len); break;
        case PARAM_OP3_LEVEL: out->min = 0.0f; out->max = 1.0f; out->default_value = 0.2f; write_str(out->name, 64, "depth", &out->name_len); break;
        case PARAM_OP4_RATIO: out->min = 0.5f; out->max = 16.0f; out->default_value = 4.0f; write_str(out->name, 64, "rate", &out->name_len); break;
        case PARAM_OP4_LEVEL: out->min = 0.0f; out->max = 1.0f; out->default_value = 0.15f; write_str(out->name, 64, "depth", &out->name_len); break;
        case PARAM_FEEDBACK: out->min = 0.0f; out->max = 1.0f; out->default_value = 0.25f; write_str(out->name, 64, "feedback", &out->name_len); break;
        case PARAM_ATTACK_MS: out->min = 0.5f; out->max = 2000.0f; out->default_value = 5.0f; write_str(out->name, 64, "attack", &out->name_len); break;
        case PARAM_DECAY_MS: out->min = 1.0f; out->max = 4000.0f; out->default_value = 150.0f; write_str(out->name, 64, "decay", &out->name_len); break;
        case PARAM_SUSTAIN: out->min = 0.0f; out->max = 1.0f; out->default_value = 0.7f; write_str(out->name, 64, "mix", &out->name_len); break;
        case PARAM_RELEASE_MS: out->min = 1.0f; out->max = 2000.0f; out->default_value = 120.0f; write_str(out->name, 64, "release", &out->name_len); break;
        case PARAM_CUTOFF_HZ: out->min = 80.0f; out->max = 20000.0f; out->default_value = 12000.0f; write_str(out->name, 64, "cutoff", &out->name_len); break;
        default: return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t op_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) return HEX_ABI_INVALID_RATE;
    OperatorState* st = (OperatorState*)calloc(1, sizeof(OperatorState));
    if (!st) return HEX_ABI_ALLOCATION_FAILED;
    st->sample_rate = sample_rate;
    st->gain = 0.5f;
    st->algorithm = 0.0f;
    st->ratio[0] = 1.0f; st->level[0] = 1.0f;
    st->ratio[1] = 2.0f; st->level[1] = 0.4f;
    st->ratio[2] = 3.0f; st->level[2] = 0.2f;
    st->ratio[3] = 4.0f; st->level[3] = 0.15f;
    st->feedback = 0.25f;
    st->attack_ms = 5.0f;
    st->decay_ms = 150.0f;
    st->sustain = 0.7f;
    st->release_ms = 120.0f;
    st->cutoff_hz = 12000.0f;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void op_destroy(void* instance) { free(instance); }

static int32_t op_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(OperatorState)) return HEX_ABI_INVALID_STATE;
    OperatorState cand;
    memcpy(&cand, bytes, sizeof(OperatorState));
    if (!isfinite(cand.gain) || cand.gain < 0.0f || cand.gain > 2.0f ||
        !isfinite(cand.feedback) || cand.feedback < 0.0f || cand.feedback > 1.0f ||
        !isfinite(cand.cutoff_hz) || cand.cutoff_hz < 80.0f || cand.cutoff_hz > 20000.0f) {
        return HEX_ABI_INVALID_STATE;
    }
    uint32_t sr = ((OperatorState*)instance)->sample_rate;
    memcpy(instance, &cand, sizeof(OperatorState));
    ((OperatorState*)instance)->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t op_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) return HEX_ABI_INVALID_ARG;
    *out_len = (uint32_t)sizeof(OperatorState);
    if (!out_bytes || capacity < sizeof(OperatorState)) return HEX_ABI_SIZE_MISMATCH;
    memcpy(out_bytes, instance, sizeof(OperatorState));
    return HEX_ABI_OK;
}

static void apply_param(OperatorState* st, uint32_t id, float v) {
    if (!isfinite(v)) return;
    switch (id) {
        case PARAM_GAIN: if (v >= 0.0f && v <= 2.0f) st->gain = v; break;
        case PARAM_ALGORITHM: if (v >= 0.0f && v <= 2.0f) st->algorithm = v; break;
        case PARAM_OP1_RATIO: if (v >= 0.5f && v <= 16.0f) st->ratio[0] = v; break;
        case PARAM_OP1_LEVEL: if (v >= 0.0f && v <= 1.0f) st->level[0] = v; break;
        case PARAM_OP2_RATIO: if (v >= 0.5f && v <= 16.0f) st->ratio[1] = v; break;
        case PARAM_OP2_LEVEL: if (v >= 0.0f && v <= 1.0f) st->level[1] = v; break;
        case PARAM_OP3_RATIO: if (v >= 0.5f && v <= 16.0f) st->ratio[2] = v; break;
        case PARAM_OP3_LEVEL: if (v >= 0.0f && v <= 1.0f) st->level[2] = v; break;
        case PARAM_OP4_RATIO: if (v >= 0.5f && v <= 16.0f) st->ratio[3] = v; break;
        case PARAM_OP4_LEVEL: if (v >= 0.0f && v <= 1.0f) st->level[3] = v; break;
        case PARAM_FEEDBACK: if (v >= 0.0f && v <= 1.0f) st->feedback = v; break;
        case PARAM_ATTACK_MS: if (v >= 0.5f && v <= 2000.0f) st->attack_ms = v; break;
        case PARAM_DECAY_MS: if (v >= 1.0f && v <= 4000.0f) st->decay_ms = v; break;
        case PARAM_SUSTAIN: if (v >= 0.0f && v <= 1.0f) st->sustain = v; break;
        case PARAM_RELEASE_MS: if (v >= 1.0f && v <= 2000.0f) st->release_ms = v; break;
        case PARAM_CUTOFF_HZ: if (v >= 80.0f && v <= 20000.0f) st->cutoff_hz = v; break;
        default: break;
    }
}

static void handle_event(OperatorState* st, const HexAbiNoteEvent* ev) {
    if (ev->kind == HEX_NOTE_EVENT_ON) {
        uint32_t slot = MAX_VOICES;
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
            if (!st->voices[v].active) { slot = v; break; }
        }
        if (slot == MAX_VOICES) slot = 0u;
        OpVoice* vc = &st->voices[slot];
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

static int32_t op_process(
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

    OperatorState* st = (OperatorState*)instance;
    double sr = (double)st->sample_rate;
    uint32_t p_idx = 0u, n_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && p_idx < ctx->param_event_count && param_events[p_idx].sample_offset <= i) {
            apply_param(st, param_events[p_idx].parameter_id, param_events[p_idx].value);
            ++p_idx;
        }
        while (note_events && n_idx < ctx->note_event_count && note_events[n_idx].sample_offset <= i) {
            handle_event(st, &note_events[n_idx]);
            ++n_idx;
        }
        float atk_step = 1.0f / fmaxf(1.0f, st->attack_ms * 0.001f * (float)sr);
        float dec_step = 1.0f / fmaxf(1.0f, st->decay_ms * 0.001f * (float)sr);
        float rel_step = 1.0f / fmaxf(1.0f, st->release_ms * 0.001f * (float)sr);
        float lpf_alpha = 1.0f - expf((float)(-TWO_PI * (double)st->cutoff_hz / sr));
        double sum = 0.0;
        for (uint32_t v = 0u; v < MAX_VOICES; ++v) {
            OpVoice* vc = &st->voices[v];
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
            double op4 = sin(vc->phase[3] + (double)(st->feedback * vc->fb_prev)) * (double)st->level[3];
            vc->fb_prev = (float)op4;
            double out_v = 0.0;
            if (st->algorithm < 0.5f) {
                double op3 = sin(vc->phase[2] + op4 * 2.0) * (double)st->level[2];
                double op2 = sin(vc->phase[1] + op3 * 2.0) * (double)st->level[1];
                out_v = sin(vc->phase[0] + op2 * 2.0) * (double)st->level[0];
            } else if (st->algorithm < 1.5f) {
                out_v = 0.25 * (sin(vc->phase[0]) * st->level[0] + sin(vc->phase[1]) * st->level[1] +
                                sin(vc->phase[2]) * st->level[2] + op4);
            } else {
                double p1 = sin(vc->phase[0] + sin(vc->phase[1]) * st->level[1] * 2.0) * st->level[0];
                double p2 = sin(vc->phase[2] + op4 * 2.0) * st->level[2];
                out_v = 0.5 * (p1 + p2);
            }
            for (int k = 0; k < 4; ++k) {
                vc->phase[k] += (TWO_PI * hz * (double)st->ratio[k]) / sr;
                if (vc->phase[k] >= TWO_PI) vc->phase[k] -= TWO_PI;
            }
            float expr = 1.0f + 0.4f * (vc->note_pressure + st->channel_pressure);
            float raw = (float)out_v * vc->velocity * vc->env * expr;
            vc->lpf_z1 += lpf_alpha * (raw - vc->lpf_z1);
            sum += (double)vc->lpf_z1;
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
    out_vtable->describe = op_describe;
    out_vtable->parameter_descriptor = op_param_desc;
    out_vtable->create = op_create;
    out_vtable->destroy = op_destroy;
    out_vtable->set_state = op_set_state;
    out_vtable->get_state = op_get_state;
    out_vtable->process = op_process;
    return HEX_ABI_OK;
}
