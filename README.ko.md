# iolite

Android 휴대폰으로 GNOME Wayland 컴퓨터를 원격 조작하는 원격 데스크톱입니다.

**알파 버전입니다.** 호스트는 Fedora 44 / GNOME 50 / Intel Core Ultra 7 155H에서, Android
앱은 CI의 에뮬레이터에서 처음부터 끝까지 시험했습니다. 버전 사이에 프로토콜이 바뀔 수 있습니다.

## 왜 만들었나

GNOME 기본 원격 데스크톱은 RDP를 쓰는데, Android용 Microsoft 클라이언트(Windows App)가
H.264를 아예 제안하지 않아서 모든 프레임이 CPU 코덱으로 처리됩니다. Fedora의 Intel 미디어
드라이버에는 애초에 H.264 인코더가 없습니다. 오디오는 꺼지고("client cannot handle graphics
and audio simultaneously"), 휴대폰 회선에서는 TCP가 멈췄다가 끊깁니다.

iolite는 GNOME에서 잘 동작하는 부분은 그대로 쓰고 나머지를 바꿨습니다.

| | |
|---|---|
| 화면·입력 | gnome-remote-desktop이 쓰는 Mutter 자체 D-Bus API. 지금 로그인한 세션을 포털 대화상자 없이 공유합니다 |
| 영상 | PipeWire DMA-BUF → GL 색 변환 → VA-API **AV1** 하드웨어 인코더. Meteor Lake에서 1920×1200 프레임당 약 2 ms |
| 소리 | 기본 출력 장치로 나가는 소리를 Opus로 |
| 전송 | UDP 위의 QUIC. 패킷 손실에도 멈추지 않고, NAT를 넘는 keep-alive와 적응형 비트레이트 |
| 클라이언트 | Android 앱. MediaCodec 저지연 모드로 AV1 하드웨어 디코딩 |
| 신뢰 | 양쪽 모두 자체 서명 인증서. 처음 한 번 6자리 코드를 비교해 페어링 |

## 호스트

GNOME Wayland(Mutter 50에서 시험), VA-API로 AV1을 인코딩하는 Intel GPU(Arc, Meteor Lake
이후), `va`·`opengl`·`pipewire`·`opus`·`videoparsers` 플러그인이 있는 GStreamer 1.24
이상이 필요합니다. Fedora에서는:

```sh
sudo dnf install gstreamer1-plugins-base gstreamer1-plugins-bad-free pipewire-gstreamer \
  libva-intel-media-driver
```

빌드하거나 최신 CI 실행의 아티팩트에서 `iolite`를 받습니다.

```sh
sudo dnf install cargo gstreamer1-devel gstreamer1-plugins-base-devel
cargo build --release -p iolite-host
install -Dm755 target/release/iolite ~/.local/bin/iolite
```

그래픽 세션 안에서 직접 실행하거나 사용자 서비스로 등록합니다.

```sh
iolite serve
# 또는
install -Dm644 dist/iolite.service ~/.config/systemd/user/iolite.service
systemctl --user enable --now iolite.service
```

UDP 46100 포트를 씁니다. 포트를 인터넷에 열지 말고 두 기기를 같은
[Tailscale](https://tailscale.com) 네트워크(또는 같은 LAN)에 두세요.

## 휴대폰

최신 CI 실행에서 `iolite.apk`를 설치하고 컴퓨터의 Tailscale 이름이나 주소를 넣어 연결합니다.
처음에는 휴대폰에 6자리 코드가, 컴퓨터에는 같은 코드가 담긴 알림이 뜹니다. 두 코드가 같을
때만 컴퓨터에서 수락하세요. 알림이 안 보이면 `iolite pending`으로 요청을 보고
`iolite approve 123456`으로 수락할 수 있습니다.

`iolite clients`는 페어링된 기기를 보여 주고, `iolite clients --forget <지문 앞부분>`은 기기를
지웁니다.

휴대폰 화면 오른쪽 가장자리의 손잡이를 누르면 키보드, 입력 방식, 연결 끊기가 나옵니다. 터치
모드에서는 손가락이 곧 포인터입니다. 탭한 곳을 클릭하고, 끌면 드래그하고, 길게 누르면
우클릭합니다. 트랙패드 모드에서는 휴대폰이 터치패드가 됩니다. 밀어서 포인터를 옮기고, 탭하면
클릭하고, 탭 후 끌면 드래그합니다. 두 모드 모두 두 손가락으로 끌면 스크롤, 두 손가락 탭은
우클릭입니다.

## 한글 입력

컴퓨터 입력기가 두벌식 ibus-hangul이면, 휴대폰에서 친 한글을 두벌식 키 입력으로 다시
보냅니다. 그래서 컴퓨터의 ibus-hangul이 지금 보고 있는 앱 안에서 바로 조합하고, 터미널에서도
똑같이 됩니다. 한/영 전환은 iolite가 알아서 하는데, 입력기 엔진을 다시 붙이는 방식이라 상단
막대의 한/영 표시가 잠깐 깜박일 수 있습니다.

## 알려진 한계

- Mutter는 컴퓨터 키보드 배열에 있는 문자만 입력할 수 있습니다. 그 밖의 문자(악센트 문자,
  이모지, ibus-hangul이 없을 때의 한글)는 클립보드에 올린 뒤 Shift+Insert로 붙여 넣고, 이때
  클립보드 내용이 바뀝니다. GTK 앱, 브라우저, Electron 앱에서는 되지만 GNOME 터미널과 Ptyxis는
  Shift+Insert에 PRIMARY 선택 영역을 붙여 넣습니다.
- 세션당 모니터 하나: 주 모니터, 또는 `iolite serve --monitor <커넥터>`.
- VA-API AV1 인코더가 필요합니다. 데스크톱용 소프트웨어 인코딩은 아직 없습니다.

## 구조

```
crates/iolite-proto    메시지, 프레이밍, 인증서, 페어링 코드
crates/iolite-client   클라이언트 세션 로직(앱과 `iolite probe`가 함께 씀)
crates/iolite-host     `iolite` 실행 파일: Mutter 세션, GStreamer, QUIC 서버
crates/iolite-android  앱용 JNI 브리지
android/               Android 앱
```

세션 하나는 QUIC 연결 하나입니다. 휴대폰이 제어 스트림을 열고 hello를 보내면, 호스트는
페어링을 요청하거나 녹화를 시작하고 프레임 크기로 답합니다. 영상(패킷마다 AV1 temporal
unit 하나)과 소리(패킷마다 Opus 패킷 하나)는 별도 스트림으로 오고, 입력은 제어 스트림으로
돌아갑니다. 한 번에 한 기기만 스트리밍하며, 새 연결이 세션을 넘겨받습니다.

`iolite probe <호스트>`는 시험용 헤드리스 클라이언트입니다. 페어링하고, 몇 초 동안 스트림을
받고, 클릭과 입력을 보내고, 받은 AV1을 파일로 저장합니다.

## 라이선스

GPL-3.0-or-later.
