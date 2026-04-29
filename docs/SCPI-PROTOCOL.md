# SCPI Protocol Reference

This document defines the SCPI-like command protocol used between the ngscopeclient `SigrokOscilloscope` driver and the sigrok bridge server, communicating over a twinlan TCP transport.

## Transport

- **Command socket** (primary): SCPI text commands, newline-delimited
- **Data socket** (secondary, port = command port + 1): binary waveform data

## Queries

| Command    | Response Format                | Description                          |
|------------|-------------------------------|--------------------------------------|
| `*IDN?`    | `vendor,model,serial,fw`     | Device identification (CSV)          |
| `CHANS?`   | `analog_count,digital_count` | Channel counts (u16 CSV)             |
| `RATES?`   | `hz1,hz2,...`                 | Available sample rates in Hz (CSV)   |
| `DEPTHS?`  | `d1,d2,...`                   | Available sample depths (CSV)        |
| `ARMED?`   | `1` or `0`                    | Whether trigger is armed             |

## Acquisition Control Commands

| Command   | Description                                          |
|-----------|------------------------------------------------------|
| `START`   | Begin continuous acquisition                         |
| `SINGLE`  | Begin single-shot acquisition (one waveform, then stop) |
| `STOP`    | Stop acquisition                                     |
| `FORCE`   | Force trigger (arm trigger immediately)              |

## Configuration Commands

| Command          | Description                              |
|------------------|------------------------------------------|
| `DEPTH <n>`      | Set sample depth to `n` samples          |
| `RATE <n>`       | Set sample rate to `n` Hz                |
| `ADC:MODE <m>`   | Set ADC mode: 0=digital, 1=8-bit analog  |

## Channel Commands

Channel commands use the hardware name prefix (e.g., `:A0:`, `:D3:`).

| Command                     | Description                                  |
|-----------------------------|----------------------------------------------|
| `:<hwname>:ON`              | Enable channel                               |
| `:<hwname>:OFF`             | Disable channel                              |
| `:<hwname>:COUP <type>`     | Set coupling (AC1M, DC1M, DC50)              |
| `:<hwname>:RANGE <volts>`   | Set voltage range                            |
| `:<hwname>:OFFS <volts>`    | Set voltage offset                           |
| `:<hwname>:THRESH <volts>`  | Set digital threshold voltage                |

## Trigger Commands

| Command              | Description                          |
|----------------------|--------------------------------------|
| `TRIG:DELAY <fs>`    | Set trigger delay in femtoseconds    |
| `TRIG:SOU <source>`  | Set trigger source channel           |
| `TRIG:LEV <level>`   | Set trigger level                    |
| `TRIG:EDGE:DIR <d>`  | Set trigger edge direction (RISING, FALLING, ANY) |

## Binary Data Format (Data Socket)

### Request

The client sends a single byte `'K'` (0x4B) to request the next waveform.

### Response: Waveform Header (38 bytes, little-endian)

| Offset | Size | Type  | Field          | Description                          |
|--------|------|-------|----------------|--------------------------------------|
| 0      | 4    | u32   | seqnum         | Waveform sequence number             |
| 4      | 2    | u16   | num_channels   | Number of channels in this waveform  |
| 6      | 8    | u64   | num_samples    | Number of samples                    |
| 14     | 8    | i64   | fs_per_sample  | Femtoseconds per sample (1e15/rate)  |
| 22     | 8    | i64   | trigger_fs     | Trigger offset in femtoseconds       |
| 30     | 8    | f64   | wfms_s         | Waveforms per second (performance)   |

### Response: Waveform Data

Data follows the header immediately, sent in chunks of 256K samples.

- **Unit size** = `ceil(num_channels / 8)` bytes per sample
- **Total data size** = `num_samples * unit_size` bytes

#### Digital Mode (ADC mode 0)

Each sample is bit-packed: channel N corresponds to bit N of the sample word.
For 16 channels, each sample is 2 bytes (little-endian).

#### Analog Mode (ADC mode 1)

Every 8 digital channels are reinterpreted as one 8-bit ADC value.
For 16 digital channels, this yields 2 analog channels per sample.
Each byte represents an unsigned 8-bit ADC reading (0-255).

Voltage conversion: `voltage = (raw_value / 255.0) * range + offset`
