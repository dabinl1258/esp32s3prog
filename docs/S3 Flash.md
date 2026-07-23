# S3 Flash

S3에 Flash하기 위해서는 다음과 같은 절차를 거처야 한다.

## Flash 모드로 진입

S3에 VDD를 인가한다.

Reset을 유지한다.

VPP를 High로 유지 한다.

## read command 사용법

Read Command를 사용 하기 위해서는 

byte1 0x61로 전송 한다.

각 바이트를 전송후 더미 클럭을 보내야 한다.

dummy clock은 1로 보내면 된다.

byte1(0x06) + byte2(addr[15:8]) + byte3(addr[7:0)

이후 byte4는 read하면 된다.
