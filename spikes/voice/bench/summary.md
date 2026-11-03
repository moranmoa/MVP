| room | netem | direction | links | latency median ms | latency p95 ms (worst link) | jitter ms | loss % | jitter buffer ms | concealed % |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| c6-clean | none | chromium -> chromium | 30 | 77 | 118 | 1.0 | 0.0 | 36 | 0.0 |
| c6-lossy | delay 50ms loss 2% | chromium -> chromium | 30 | 120 | 156 | 1.0 | 2.1 | 31 | 1.9 |
| c6-jitter | delay 50ms 10ms distribution normal loss 2% | chromium -> chromium | 30 | 154 | 194 | 10.0 | 2.0 | 62 | 1.9 |
| s6-clean |  | str0m -> str0m | 30 | 18 | 37 | 0.1 | 0.0 | 20 | 0.2 |
| s6-lossy | delay 50ms loss 2% | str0m -> str0m | 30 | 68 | 76 | 0.0 | 2.0 | 20 | 2.1 |
| s6-jitter | delay 50ms 10ms distribution normal loss 2% | str0m -> str0m | 30 | 70 | 117 | 11.3 | 1.9 | 24 | 14.1 |
| mix-lossy | delay 50ms loss 2% | chromium -> chromium | 6 | 126 | 142 | 1.0 | 2.0 | 37 | 1.9 |
| mix-lossy | delay 50ms loss 2% | chromium -> str0m | 9 | 119 | 136 | 0.4 | 0.0 | 45 | 2.2 |
| mix-lossy | delay 50ms loss 2% | str0m -> chromium | 9 | 78 | 101 | 0.0 | 2.1 | 15 | 1.4 |
| mix-lossy | delay 50ms loss 2% | str0m -> str0m | 6 | 68 | 91 | 0.1 | 2.1 | 20 | 2.3 |
| c6-mic-apm | none | chromium -> chromium | 30 | n/a | n/a | 0.0 | 0.0 | 30 | 0.0 |
| c6-mic-raw | none | chromium -> chromium | 30 | n/a | n/a | 0.0 | 0.0 | 30 | 0.0 |
| s6-apm |  | str0m -> str0m | 30 | n/a | n/a | 0.1 | 0.0 | 20 | 0.0 |

| room | variant | participants | source | CPU % of one core, median | max | container memory MiB |
| --- | --- | --- | --- | --- | --- | --- |
| c6-clean | chromium | 6 | pulses | 40.4 | 40.8 | 154 |
| c6-lossy | chromium | 6 | pulses | 40.9 | 41.8 | 159 |
| c6-jitter | chromium | 6 | pulses | 40.5 | 40.5 | 160 |
| s6-clean | str0m | 6 | pulses | 5.6 | 5.9 | 4 |
| s6-lossy | str0m | 6 | pulses | 4.3 | 4.3 | 5 |
| s6-jitter | str0m | 6 | pulses | 4.6 | 4.7 | 4 |
| mix-lossy | chromium | 3 | pulses | 42.1 | 42.5 | 158 |
| mix-lossy | str0m | 3 | pulses | 5.4 | 5.5 | 4 |
| c6-mic-apm | chromium | 6 | mic | 36.5 | 37.6 | 156 |
| c6-mic-raw | chromium | 6 | mic | 32.7 | 33.8 | 154 |
| s6-apm | str0m | 6 | pulses+apm | 6.5 | 6.5 | 5 |
