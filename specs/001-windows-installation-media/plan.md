# Implementation Plan: M5 — Windows installation media

**Branch**: `main` (Git real; nenhuma branch criada) | **Date**: 2026-10-21 | **Spec**: [spec.md](spec.md)

**Input**: `specs/001-windows-installation-media/spec.md`; identificador ativo retornado pelo Spec Kit: `001-windows-installation-media`.

## Summary

Adicionar criação Windows UEFI em FAT32 com WIM dividido e NTFS com partição auxiliar UEFI:NTFS. Seleção automática prioriza FAT32; escolha manual permanece disponível. Reutilizar segurança, RawIo, progresso, CLI e GUI existentes. Preparar transformações e validar capacidade antes da primeira escrita no destino. Não alterar firmware, ISO original ou registro do hospedeiro.

Pesquisa concluída como desenho, não como prova de execução. Provas nativas e de boot abaixo são etapas obrigatórias da implementação; nenhuma ferramenta externa foi instalada, nenhum dispositivo foi gravado.

**Ordem de entrega aprovada**: macOS primeiro. Implementar e testar no Mac atual; preservar fronteiras portáveis, sem exigir cross-compilação ou runners Linux/Windows para avançar neste incremento. O usuário compilará esses hosts separadamente. Manter suas validações pendentes, sem alegar suporte comprovado. Gates de segurança, integridade e boot da mídia Windows permanecem obrigatórios; o incremento macOS não equivale à conclusão multiplataforma do M5.

## Technical Context

**Language/Version**: Rust edition 2024; MSRV 1.88; toolchain fixada em 1.98.1. C apenas para componentes nativos e adaptadores mínimos.

**Primary Dependencies**: Manter fatfs, sha2, serde, clap, egui/eframe e tempfile existentes. Acrescentar libudfread para leitura UDF com offsets de 64 bits; libntfs-3g e adaptação de mkntfs para NTFS sem montagem; wimlib-imagex 1.14.5 entregue como recurso privado por plataforma. Dependências nativas devem ter versão/commit, SHA-256, fontes e receita de build fixados antes da integração; não usar branches móveis.

**Storage**: ISO read-only, snapshot privado integral para extração Windows, arquivos preparados e destino RawIo existente. Reservar scratch para snapshot e transformações. Sem banco, montagem NTFS, FUSE ou download em runtime.

**Testing**: cargo test, Clippy, rustfmt; testes sintéticos nas suítes existentes; validação nativa das bibliotecas; leitura independente NTFS/chkdsk, QEMU/OVMF, hardware autorizado. Testes reais usam ISOs fornecidas externamente, não distribuídas no repositório.

**Target Platform**: criação em macOS Intel/Apple Silicon, Linux x64/ARM64 e Windows x64/ARM64; validação mínima Windows 10 22H2 x64 e Windows 11 24H2 x64, registrando edição, build e SHA-256 de cada ISO. Ajustes só anunciados após validação na build exata. Windows 11 ARM64 condicionado à disponibilidade, explicitamente não validado até haver evidência.

**Project Type**: workspace Rust com biblioteca, CLI e desktop existentes. Não adicionar serviço, framework de testes ou crate de domínio novo.

**Performance Goals**: leitura/cópia em blocos limitados, sem carregar WIM/ISO na RAM; atualização de progresso com intervalo de aproximadamente 100 ms já existente. Sem paralelismo configurável ou promessa de throughput sem benchmark.

**Constraints**: execução offline; nenhuma dependência manual instalada pelo usuário; recursos auxiliares entregues no bundle/executável. Chamadas externas por caminho absoluto e argumentos, nunca shell ou PATH. Segurança de destino preservada. Secure Boot validado com DB/DBX e certificados registrados; não há compatibilidade universal.

**Scale/Scope**: um trabalho de gravação por aplicação, até 250.000 entradas como teto atual, tamanhos/offsets u64 verificados. FAT32 e NTFS, MBR/GPT existentes ampliados para duas partições. Sem BIOS Windows, conversão ESD, download ISO ou instalação automática em disco.

## Constitution Check

Arquivo de constituição contém apenas placeholders; nenhuma regra ratificada é inventada. Gates derivados de SECURITY.md e ROADMAP.md:

| Gate | Antes da pesquisa | Após desenho |
|---|---|---|
| Confirmação, bloqueio de disco de sistema, identidade revalidada | PASS | PASS: abrir destino somente após preparação e confirmação |
| Entrada ISO não confiável, caminhos e limites verificados | PASS | PASS: catálogo limitado, offsets checked, sem links/traversal |
| Unsafe restrito a platform | PASS | PASS: FFI e callbacks encapsulados em platform |
| Nenhuma execução de conteúdo da ISO no hospedeiro | PASS | PASS: helpers próprios fixados; reg.exe apenas no WinPE durante setup |
| Licenças e fontes distribuíveis | PASS como requisito | PASS no desenho; auditoria dos artefatos bloqueia distribuição |
| Sem regressão Linux/DD | PASS como requisito | PASS no desenho; suíte existente bloqueia merge |

Não há violação conhecida de desenho. Falha em qualquer prova de viabilidade abaixo interrompe a implementação desse caminho e exige revisão explícita do plano, não redução silenciosa de escopo.

## Project Structure

### Documentation (this feature)

```text
specs/001-windows-installation-media/
  spec.md
  plan.md
  research.md
  data-model.md
  contracts/windows-media.md
  quickstart.md
  checklists/requirements.md
```

`tasks.md` será gerado por `/speckit.tasks`, não por este comando.

### Source Code (repository root)

```text
crates/core/src/plan.rs                 # seleção e opções independentes
crates/core/src/progress.rs             # fase Preparing
crates/platform/src/media.rs            # fronteira segura UDF/NTFS e helpers
crates/platform/native/                 # shim C mínimo e integração formatter
crates/iso/src/{analyze,iso9660}.rs      # catálogo u64/múltiplos extents
crates/extract/src/windows.rs           # preparar, selecionar, transformar
crates/extract/src/{extract,layout,gpt}.rs
crates/cli/src/main.rs
crates/gui/src/main.rs
crates/iso/tests/synthetic.rs
crates/extract/tests/extract.rs
assets/uefi-ntfs/                       # imagem upstream assinada intacta
scripts/                               # build reproduzível recursos nativos
.github/workflows/{ci,release}.yml
docs/validation/                        # evidências reais, nunca sucesso simulado
```

**Structure Decision**: reutilizar crates atuais. `iso` e `extract` podem depender da API segura de mídia em `platform`, que depende apenas de `core`; não criar ciclo. Separar em crate nativo só se essa dependência impedir compilação ou manutenção, mediante revisão.

## Phase 0 — Research decisions

Ver [research.md](research.md). Decisões: libudfread, wimlib-imagex privado, NTFS-3G via callbacks e formatter adaptado, imagem UEFI:NTFS upstream assinada intacta, personalização por answer file sem seleção automática de disco. Sem decisões de produto pendentes; suporte de builds específicos continua condicionado a testes.

## Phase 1 — Design

1. **Provas de viabilidade primeiro**: compilar leitor UDF e escritor/formatter NTFS em cada host; criar NTFS com diretórios e arquivo >4 GiB, verificar com Windows; validar o helper WIM por arquitetura. Registrar versões e fontes exatas. Se o formatter não operar em RawIo com segurança, parar e revisar, sem implementar NTFS próprio.
2. **Catálogo e plano comuns**: ler UDF quando presente em Windows; suportar ISO9660 multi-extent corretamente, sem promover apenas um campo u32. Fazer detecção e extração usarem o mesmo catálogo e arquivos de 64 bits. Verificar PE machine do fallback EFI e consistência com imagens WIM.
3. **Preparação**: criar snapshot privado integral da ISO antes da análise definitiva e preparação; todas as leituras posteriores usam esse snapshot. Manter origem aberta read-only durante captura, comparar identidade e metadados antes/depois e abortar se houver mudança detectada. Calcular scratch para snapshot e transformações, com cleanup em falha/cancelamento. Snapshot estabiliza bytes usados, mas não comprova autenticidade nem garante detectar adulterações adversariais durante captura. Validar SWM/WIM/ESD, gerar partes/answer file e calcular manifesto esperado. Verificar partes reais abaixo do limite FAT32. Auto usa NTFS se formato/recurso impedir divisão válida; informar motivo antes de confirmar. FAT32 manual falha sem fallback silencioso.
4. **Layout**: FAT32 conserva layout de uma partição. NTFS usa primeira partição de dados (MBR 0x07 ou GPT Microsoft Basic Data) e última partição FAT auxiliar com imagem UEFI:NTFS upstream (MBR 0xEF ou GPT ESP), tamanho derivado do asset fixado e alinhamento de 1 MiB. Atualizar CRCs/GPT backup; validar capacidade e geometria antes de escrever. Sem instalação GRUB no caminho Windows.
   A geometria interna do asset, incluindo BPB, deve ser compatível com o setor lógico do destino e validada em teste. Alinhamento externo não comprova compatibilidade 4Kn. Recusar combinações incompatíveis ou não comprovadas antes de gravar, sem alterar o asset assinado; preservar FAT32 4Kn existente.
5. **Commit**: mostrar plano final, exigir confirmação, revalidar recurso privado preparado e identidade/capacidade do dispositivo, abrir exclusivamente, escrever layout e arquivos a partir do snapshot, flush, releitura quando solicitada, invalidar/ejetar. Alteração posterior na ISO original não modifica o snapshot usado. Qualquer erro após commit marca destino incompleto. Não confundir falha de ejeção com falha de cópia ou sucesso integral.
6. **Personalização**: experimentar mecanismos em mídia temporária e VM descartável Windows 11 24H2 x64, registrando edição/build/hash e rede on/off antes de gerar código de produção; isso não usa nem depende do gerador de answer file de produção. Implementar três flags separadas e root autounattend.xml apenas após comprovação; validar novamente o fluxo integrado. Não criar conta/password administrador automaticamente. Rejeitar answer files conflitantes; preservar seleção interativa de disco. Consultar orientação Secure Boot é ação de ajuda, não flag de modificação.
7. **Integração**: CLI/GUI chamam a mesma preparação/plano; retirar rejeição Windows somente quando o caminho correspondente estiver implementado. `--dry-run` executa validações e preparação temporária necessárias, nunca altera o destino; mostra custo de scratch.

Contratos e entidades: [contracts/windows-media.md](contracts/windows-media.md), [data-model.md](data-model.md). Validação: [quickstart.md](quickstart.md).

## Validation and release gates

- Testes do limite FAT32 em max e max+1; fragmentação UDF e ISO multi-extent, overflow, traversal, colisões, SWM faltante, recurso WIM indivisível, cancelamento e erro de helper.
- Para cada falha de pré-processamento, comparar hash integral do arquivo destino antes/depois; proteger origem igual ao destino, symlink e hard link.
- Confirmar espaço temporário para snapshot integral e transformações, além da capacidade com arquivos realmente preparados; erros durante preparação nunca tocam destino. Mudança detectada durante captura aborta; mudança posterior na origem não afeta o plano baseado no snapshot privado. Testar falta de espaço, limpeza e alteração concorrente durante/depois da captura, inclusive alteração de conteúdo sem mudança de tamanho.
- Verificar cada arquivo transformado com hash de saída esperado; NTFS por leitor independente e Windows chkdsk. Sem sucesso se houver divergência ou metadados inválidos.
- Matriz: Windows 10/11 x64 × FAT32/NTFS × Secure Boot on/off × VM/físico. Registrar ISO SHA-256, revisão, DB/DBX, assinaturas, firmware, opções e logs/screenshots. Em pelo menos um WIM grande, iniciar aplicação de uma edição em disco descartável da VM.
- ARM64, limitações de cancelamento em chamadas nativas e builds de Windows suportados explicitamente documentados. Falta de ambiente é pendência, não teste aprovado.
- Executar os três comandos de qualidade e cross-checks do ROADMAP; acrescentar builds nativos dos componentes na CI. Atualizar README, ROADMAP, compatibility, SECURITY e THIRD_PARTY_NOTICES com fontes hospedadas junto à distribuição.

## Complexity Tracking

| Complexidade necessária | Motivo | Alternativa descartada |
|---|---|---|
| UDF e NTFS nativos com shim C | Imagens Windows grandes; NTFS explicitamente solicitado | Bibliotecas já instaladas não escrevem NTFS nem leem UDF; escritor NTFS próprio excessivamente arriscado |
| Helper WIM privado | Ferramenta madura, compilação Windows MinGW isolada do Rust MSVC | FFI WIM amplia ABI/unsafe e obrigações de relink sem benefício inicial |
| Duas partições em NTFS | Firmware UEFI precisa acessar carregador FAT antes de NTFS | Montagem pelo host/FUSE exige instalação; NTFS puro não é universalmente legível por firmware |
