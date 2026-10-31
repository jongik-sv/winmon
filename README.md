# winmon

Windows용 성능 모니터. [macmon](https://github.com/vladkens/macmon) 에서 영감을 받았다.
관리자 권한·드라이버 없이 Windows 기본 성능 카운터(PDH)만 읽는다.

- **코어별 CPU 사용률 그래프** — P코어·E코어를 구분해 코어마다 사용률·실효 클럭을 시간축 그래프로 보여 준다. P코어는 왼쪽, E코어는 오른쪽 열에 모아 둔다
- **전력** — 패키지·CPU 코어·GPU·DRAM (Intel RAPL, `Energy Meter` 카운터)
- **GPU·NPU 사용률** — 3D·Compute·Video 엔진, NPU (작업 관리자와 같은 데이터)
- **메모리** — RAM, 커밋, 페이지 파일(swap)
- **온도** — ACPI 열 구역(CPU·표면·충전부 등)

```
┌ CPU 46% ──────────────────────────────────────────────────────────────┐
│┌ P0 ─────── 58% 2.42 GHz ┐┌ P1 ────── 40% 2.22 GHz ┐┌ E0 ─ ...        │
││                    ███▇ ││                      ▅ ││                 │
││                   █████ ││                   █▂▆█ ││                 │
│└─────────────────────────┘└────────────────────────┘└─                │
┌ Power ────────────────────────┐┌ GPU · NPU ──────────────────────────┐
│ Package      7.67 W     ▆▇▆▇█ ││ GPU 3D           0%                 │
│ CPU cores    5.09 W     ▅▇▆▇█ ││ NPU              0%                 │
```

### 화면 배치

창 크기에 따라 CPU 칸이 바뀐다. 어느 배치에서든 P코어는 왼쪽, E코어는 오른쪽이다.

- **넓고 높은 창** — 코어마다 그래프 칸. 한 줄에 다 들어가지 않으면 `P0 P1 │ E0 E1` / `P2 P3 │ E2 E3` 처럼 줄을 나눈다
- **좁은 창** — 왼쪽 열에 P0~P3, 오른쪽 열에 E0~E3 그래프 칸
- **낮은 창** — 코어당 한 줄짜리 작은 그래프로 바꾼다

```
│ P0   ▁ ▁  15% 2.56 GHz E0   ▅▅▇▆█ 100% 2.36 GHz│
│ P1 ▂▆▁ ▆  81% 2.62 GHz E1   ▆▇▇▄█ 100% 2.37 GHz│
│ P2 ▁   ▂  28% 2.43 GHz E2   ▅▆▇▅█ 100% 2.34 GHz│
│ P3   ▁     6% 2.46 GHz E3   ▆▆▇▅█ 100% 2.34 GHz│
```

창이 더 낮으면 CPU 칸을 지키려고 아래 칸부터 숨긴다. Memory·Temperature 칸이 먼저, 그다음 Power·GPU·NPU 칸이 사라진다. 전력이 안 보이면 창을 키우면 된다.

## 실행

```
winmon                 # 1초 간격
winmon -i 500          # 0.5초 간격 (최소 250ms)
winmon --once 120x40   # 한 화면을 텍스트로 출력하고 끝낸다
```

키: `q`/`Esc` 끝내기, `+`/`-` 측정 간격 ±250ms.

Windows Terminal 에서 쓰는 것을 권장한다(옛 콘솔 창은 그래프 문자 `▁▂▃▅▇█` 가 깨질 수 있다).

winmon 자체의 부하는 작다. 1초 간격에서 CPU 는 코어 하나 기준 약 0.2%, 메모리는 약 14MB, 스레드 1개다.

## 설치

어느 폴더에서든 `winmon` 으로 실행하려면 프로젝트 폴더에서 설치한다.

```
cargo install --path .
```

`%USERPROFILE%\.cargo\bin\winmon.exe` 에 설치되고, 이 폴더는 rustup 이 PATH 에 넣어 둔다.
코드를 고친 뒤에는 같은 명령으로 다시 설치한다. 이때 winmon 이 켜져 있으면 exe 를 덮어쓸 수 없어(액세스 거부) 먼저 끝내야 한다.

다른 PC 에는 `winmon.exe` 하나만 복사하면 된다. Windows 기본 DLL 만 쓴다.

## 빌드

Visual Studio Build Tools 없이 빌드하도록 GNU 툴체인(`x86_64-pc-windows-gnu`)을 쓴다(`.cargo/config.toml`).

1. Rust: `rustup-init.exe --default-host x86_64-pc-windows-gnu`
2. `dlltool.exe`(MinGW-w64 binutils)를 PATH 에 둔다. 예: MSYS2 의 `ucrt64\bin`
3. `cargo build --release` → `target\x86_64-pc-windows-gnu\release\winmon.exe`

결과물은 단일 exe(약 600KB)이고, 다른 PC 에서 실행할 때는 Rust·MinGW 가 필요 없다.

## 한계

| 항목 | 내용 |
|---|---|
| 코어별 온도 | 없다. 커널 드라이버(관리자 권한)가 필요해 넣지 않았다. ACPI 열 구역은 펌웨어가 정한 센서라 PC 마다 이름·개수가 다르다 |
| 전력 | `Energy Meter` 카운터(RAPL)를 펌웨어가 노출해야 나온다. 없는 PC 는 칸에 "not available" 로 표시 |
| GPU 클럭 | 표준 카운터에 없어 넣지 않았다 |
| E코어·LP E코어 | Windows 가 같은 효율 등급으로 보고하면 구분할 수 없어 둘 다 E 로 표시 |
| 사용률 | `% Processor Utility` 를 쓴다(작업 관리자와 같다). 터보 중 100% 를 넘는 값은 100% 로 자른다 |

## 라이선스

[MIT](LICENSE)
