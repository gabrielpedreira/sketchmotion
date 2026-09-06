# Backlog de ideias

Ideias levantadas durante o desenvolvimento, para implementar **fora da ordem**
do roadmap principal ([`05-roadmap-mvp.md`](./05-roadmap-mvp.md)). Cada uma é
detalhada e encaixada numa versão quando for priorizada — registrar aqui evita
perder a ideia sem interromper o trabalho em andamento.

## Em andamento

### Paletas por personagem (crate `color`)
Paletas nomeadas por área do personagem (Pele, Roupa, Cabelo...) com cores de
rótulo livre (base, luz, sombra...) e código hex, para manter consistência do
personagem entre frames e entre projetos.
- [x] Parte A — estrutura de dados + hex + testes
- [x] Parte B — seletor de cores abrangente (clique + código hex)
- [ ] Parte C — painel de gerenciamento (criar personagem/grupo/cores, pintar a partir da paleta)
- [ ] Parte D — persistência (biblioteca global reutilizável + embutida no projeto)

## Backlog (ainda não iniciado)

### Modo pixel art
Canvas em baixa resolução de pixels lógicos escolhida pelo usuário
(ex.: 16/32/64 px = nível de detalhamento), exibido ampliado com **grade**
sobreposta guiando as células; lápis pinta **1 pixel lógico** por clique
(encaixado na grade, sem antisserrilhado).

### Tipos de pincéis e texturas
Ampliar o sistema de pincéis (crate `brush`) para além do lápis sólido:
- Pincéis de tinta/nanquim (inking), com variação de espessura no traço.
- Aquarela / pincéis com esmaecimento (opacidade e bordas suaves).
- Spray / aerógrafo (dispersão de pontos).
- Pincéis com **textura** (a "ponta" do pincel é uma imagem/máscara aplicada ao longo do traço).
- Arquitetura deve permitir **adicionar novos pincéis** sem alterar os existentes
  (cada pincel implementa um trait comum), e futuramente importar pincéis.
