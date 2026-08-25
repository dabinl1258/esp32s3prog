# 실행 방법
## 1. rust 설치
https://rustup.rs/#
위 페이지에서 rustup-init.exe 파일을 받아서 설치

## 2. risc-v 타겟 추가
터미널에 아래 명령어를 입력 하여 risc-v 타겟 추가
```
rustup target add riscv32imac-unknown-none-elf
```

## 3. 소스 다운로드
```
git clone -b wifi_ver https://github.com/dabinl1258/esp32s3prog.git
```

## 4. wifi ssid 비밀번호 입력
ESP32C6에서 네트워크를 접속 하기 위한 wifi ssid(이름)  비밀번호를 입력

*2.4GHz 만 지원

## 5. espflash 설치
esp32c6에 프로그램 넣기 위해서 espflash 설치
```
cargo binstall cargo-espflash 
```

## 6. 컴파일 및 실행
```
cargo run -- --baud 921600 
```
wifi에  연결 된 경우,  터미널 창에서 ip 정보 확인 가능 
