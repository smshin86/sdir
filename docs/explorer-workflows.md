# Windows 탐색기 기본 동작 기준

Windows 탐색기에서 관찰한 동작을 SDIR의 기존 파일 관리 기능과 비교한다. 동일한 임시 폴더와 파일로 입력, 화면의 반응, 실제 파일 결과를 확인한다.

## Windows에서 확인한 결과

확인한 탐색기는 Windows 빌드 26300에서 실행 중인 `explorer.exe` 버전 10.0.26100.9949다. REA가 PE x64 파일을 분석한 SHA-256은 Windows에서 계산한 `eee840f2bce235dce1da0a5f4eacdb94a2bfb11acf13822d7dbcd243273219c3`와 일치했다. 파일 조작은 Windows에서 열린 탐색기 창을 실제로 조작해 확인했다.

| 입력 | 확인한 결과 | 증거 |
|---|---|---|
| 이름순으로 열린 폴더에 중간 이름의 파일을 외부에서 생성 | 새 파일이 현재 목록의 마지막에 나타났다. | [탐색기 목록 순서](task-id/index-management/production-fresh-Xp4LAP2R/explorer-reference/explorer-order.json) |
| F5를 누르거나 하위 폴더에 들어갔다가 뒤로 이동 | 현재 정렬에 맞는 위치에 새 파일이 나타났다. | [탐색기 목록 순서](task-id/index-management/production-fresh-Xp4LAP2R/explorer-reference/explorer-order.json) |
| 같은 볼륨 안에서 폴더 행에 드롭 | 대상에 파일이 생기고 원본 위치에서 사라졌다. | [탐색기 드래그](task-id/index-management/production-fresh-Xp4LAP2R/explorer-reference/explorer-drag.json) |
| 다른 볼륨의 폴더 행에 드롭 | 대상에 같은 내용의 파일이 생기고 원본이 남았다. | [탐색기 드래그](task-id/index-management/production-fresh-Xp4LAP2R/explorer-reference/explorer-drag.json) |
| Ctrl을 누르고 같은 볼륨의 폴더 행에 드롭 | 대상에 복사했고 원본이 남았다. | [탐색기 드래그](task-id/index-management/production-fresh-Xp4LAP2R/explorer-reference/explorer-drag.json) |
| Shift를 누르고 다른 볼륨의 폴더 행에 드롭 | 같은 내용의 파일이 대상으로 이동했다. | [탐색기 드래그](task-id/index-management/production-fresh-Xp4LAP2R/explorer-reference/explorer-drag.json) |

## SDIR 적용 및 검증

- Windows 이름순은 숫자를 숫자값으로 읽고 기호는 Windows 비교 순서를 따른다. Windows Shell의 파일·폴더 비교를 기준으로 `★`, `!`, `1·2·10`, 한글, 같은 모양으로 표시되는 유니코드 이름을 확인했다. 실제 SDIR에서 폴더 열기, 정렬 방향 전환, 외부 추가, F5와 재진입 여정 일곱 가지를 검증했다. [Windows 비교](task-id/index-management/explorer-name-sort-uQ5SpYW6/native-pairs.json) · [SDIR 사용자 여정](task-id/index-management/explorer-name-sort-uQ5SpYW6/verified/sdir-order.json)
- J35에서 실제 마우스로 끄는 중 파일 표시의 위치 변화와 대상 강조, 드롭 후 표시 제거를 확인했다. 같은 폴더의 하위 폴더, 다른 패널, 다른 탭의 제목과 탭 안의 폴더에서 복사·이동 결과를 여섯 건 대조했다. 다른 탭의 제목에 놓은 파일은 대상 탭에서 선택됐다. [사용자 여정 기록](task-id/index-management/drag-preview-HwfE1xB3/verified/folder-drop-journey.json) · [어두운 모드 화면](task-id/index-management/drag-preview-HwfE1xB3/verified/drag-in-progress.png)
- 같은 Windows 빌드에서 외부 앱에서 SDIR로 파일을 가져오고, SDIR에서 나란히 있거나 겹쳐 있는 Windows 파일 선택 창으로 보내 실제 대상 내용과 원본 보존을 확인했다. [가져오기](task-id/index-management/drag-preview-HwfE1xB3/inbound/folder-drop-journey.json) · [나란한 외부 창](task-id/index-management/drag-preview-HwfE1xB3/outbound-adjacent/folder-drop-journey.json) · [겹친 외부 창](task-id/index-management/drag-preview-HwfE1xB3/outbound-overlap/folder-drop-journey.json)

- 폴더를 연 채로 항목이 추가되면 기존 순서와 선택을 유지하고 새 항목을 끝에 표시한다. F5와 폴더 재진입 시 현재 정렬을 적용한다. 실제 폴더를 외부에서 바꾸고 선택, 포커스, 이름 변경과 삭제도 함께 확인했다. [소규모 목록](task-id/index-management/post-review-final-mWVAbE/order/sdir-order.json) · [가상 목록](task-id/index-management/post-review-final-mWVAbE/full-list/explorer-journey.json)
- SDIR 안의 폴더 행에 파일을 놓으면 그 폴더에 복사하거나 이동한다. 실제 마우스로 같은 볼륨과 다른 볼륨, Ctrl과 Shift를 시험하고 대상 바이트와 원본 보존 여부를 확인했다. [SDIR 폴더 드롭](task-id/index-management/post-review-final-mWVAbE/internal/folder-drop-journey.json)

- 같은 폴더를 2번과 3번 패널에 열어 두고 실제 마우스로 3번에 놓았다. 파일은 양쪽에 표시됐고 선택과 키보드 초점은 3번 패널에만 적용됐다. [중복 폴더 드롭](task-id/index-management/post-review-final-mWVAbE/duplicate/folder-drop-journey.json)
- 첫 버튼을 놓은 상태를 실제 Windows 자식 프로세스에 전달했을 때 작업자가 취소 응답을 보내고 정상 종료했다. 원본 바이트도 보존됐다. [작업자 취소](task-id/index-management/post-review-final-mWVAbE/drag-worker-native/drag-worker.json)

- Windows의 별도 시험 데스크톱에서 SDIR의 파일을 Windows 파일 선택 창으로 드래그했다. 창이 나란히 있을 때와 겹쳐 있을 때 모두 대상 파일의 내용이 원본과 일치했고 원본 파일이 남았다. 두 시험의 실행 파일 해시는 배포 후보와 같았고 종료 후 원래 화면으로 복귀했다. [나란히 놓인 창](task-id/index-management/final-desktop-check-50mrOu/outbound/folder-drop-journey.json) · [겹친 창](task-id/index-management/final-desktop-check-50mrOu/overlap/folder-drop-journey.json)

이미 설치된 색인 서비스로 C·D·E에서 생성, 이름 변경, 삭제가 검색에 반영되는 것을 확인했다. 이번 후보의 설치 시험은 기존 서비스 상태에서 앱 신규 설치와 업데이트, 제거를 확인했다. 색인 서비스까지 처음 설치하고 저장된 전체 드라이브 색인을 재실행 후 복원하는 시험에는 별도 Windows 환경이 필요하다. [실시간 검색 기록](task-id/index-management/post-review-final-mWVAbE/all-drives/worker-live-journey.json)

## 기존 기능 수정에 적용할 검증 순서

1. 탐색기에서 입력부터 결과까지 재현한다. 탐색·선택·파일 작업에서 누른 키, 클릭하거나 놓은 위치, 대상의 상태를 기록한다.
2. 같은 시나리오를 배포 후보의 SDIR에서 실행한다. 열린 폴더와 폴더 행, 여러 패널·탭에서 실제 사용자가 지정한 대상을 확인한다. 선택, 키보드 초점, 스크롤, 목록 순서와 실제 파일 내용을 함께 확인한다.
3. 차이가 있으면 재현 시험을 만든 뒤 해당 흐름을 수정한다. 복사와 이동은 원본 보존 여부, 덮어쓰기와 취소는 양쪽 파일 내용, 자동 갱신은 F5 없이 추가·변경·삭제가 반영되는지까지 검증한다.
4. 동일한 입력을 새 빌드에서 다시 실행한다. 최종 배포 파일에 포함된 실행 파일 해시와 검증한 실행 파일 해시를 대조한다.

키보드 입력과 우클릭, 파일 작업·실행 취소, 검색 결과 이동, 환경 변화에 대한 기존 검증 범위는 [SDIR 사용자 여정](user-journeys.md)을 따른다.
