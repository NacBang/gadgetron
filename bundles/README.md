# Bundle source map

Gadgetron의 공식 제품 용어는 다음 두 가지다.

- **Operational Bundle**(기능 번들): domain 상태·관측·action·검증·rollback·outcome을 소유한다.
- **Intelligence Bundle**(지식 번들): source·ontology·수집·연구·증류·lesson·insight를 소유한다.

signed `package.template.toml`의 `[bundle].class`가 구현된 제품 class의 단일 권위다. 디렉터리명,
crate 이름, Gadget 이름이나 UI 위치로 class를 추론하지 않는다.

## Product packages

| Package | Class | 소유권 경계 |
|---|---|---|
| `server-administrator` | Operational | 서버 등록·bootstrap·관측·로그·복구·outcome |
| `travel-planner` | Operational | Trip·일정·제약·예산·re-plan·outcome |
| `restaurant-research` | Intelligence | 음식점 source·snapshot·recommendation·visit lesson |
| `server-operations-intelligence` | Intelligence | 서버 vendor/community/runbook source·lesson·운영 적용 근거 |
| `travel-intelligence` | Intelligence | destination/advisory/option source·lesson·여행 적용 근거 |
| `news-intelligence` | Intelligence | Topic·article snapshot·claim/correction·briefing |
| `community-intelligence` | Intelligence | 공식 community API snapshot·discussion·solution pattern·evidence |

Operational package는
Core의 `IntelligenceQuery -> KnowledgeContextPack -> OutcomeFeedback` 교환만 사용하며 Intelligence
package의 code, private API, table 또는 Gadget을 직접 호출하지 않는다.

## Non-product compatibility code

| Directory | 역할 |
|---|---|
| `document-formats` | Knowledge ingest port의 extractor/Plug provider compatibility crate |
| `gadgetron-core` | Core built-in workbench descriptor의 legacy catalog fixture |

과거 server-monitor/log-analyzer source는 Server Administrator 기능 parity 뒤 제거됐으며 Git 이력에만
남는다. 위 디렉터리는 독립 설치 제품이 아니며 signed package catalog에 노출하지 않는다. 새 제품 Bundle은
독립 package/version/runtime/data lifecycle과 public SDK 경계를 가져야 한다.
