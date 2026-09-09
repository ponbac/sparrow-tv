"""Summarize completed, sanitized installed-probe frame pacing observations."""
import argparse
import json
import pathlib
import statistics


def summarize(samples, start):
    first = next(sample for sample in samples if sample['elapsed'] == start)
    last = samples[-1]
    steady = [sample for sample in samples if sample['elapsed'] > start]
    if not steady:
        raise ValueError('Run must extend beyond the steady interval start')
    return {
        'interval_seconds': [start, last['elapsed']],
        'startup_seeks': first['seeking'],
        'steady_seeks': last['seeking'] - first['seeking'],
        'steady_waiting_events': last['waiting'] - first['waiting'],
        'approximate_callbacks_per_second': round(
            (last['presented'] - first['presented']) / (last['elapsed'] - start), 2),
        'maximum_frame_callback_gap_ms': max(sample['frameGaps']['max'] for sample in steady),
        'maximum_animation_callback_gap_ms': max(sample['rafGaps']['max'] for sample in steady),
        'sample_intervals_with_frame_gap_over_100ms': sum(
            sample['frameGaps']['max'] > 100 for sample in steady),
        'median_forward_buffer_seconds': round(statistics.median(
            sample['bufferAhead'] for sample in steady), 2),
        'decoder_counter_resets': sum(
            sample['total'] < previous['total'] for previous, sample in zip(samples, samples[1:])),
        'failed_samples': sum(sample['failed'] for sample in samples),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('samples', type=pathlib.Path, nargs='+')
    parser.add_argument('--start', type=int, default=20)
    args = parser.parse_args()
    results = []
    for path in args.samples:
        samples = json.loads(path.read_text())
        results.append({'file': str(path), **summarize(samples, args.start)})
    print(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
