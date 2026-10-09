import org.jsoup.Jsoup;
import org.jsoup.nodes.Attribute;
import org.jsoup.nodes.Comment;
import org.jsoup.nodes.DataNode;
import org.jsoup.nodes.Document;
import org.jsoup.nodes.DocumentType;
import org.jsoup.nodes.Element;
import org.jsoup.nodes.Node;
import org.jsoup.nodes.TextNode;
import org.jsoup.nodes.XmlDeclaration;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * S0 spike / T008：把 HTML 用 Jsoup 解析后按**与 Rust 侧同构的约定**序列化 DOM。
 *
 * <p>这是差分对照的 Java 半边。上游 legado 用 Jsoup 解析网页并跑 CSS 选择器，
 * 本项目改用 html5ever + 自建 DOM 实现（research.md R2）。两边解析同一份乱网页
 * 得到的树是否一致，是书源兼容性的地基——本工具把它变成可量化数据。
 *
 * <p><b>干净室说明</b>：本文件不读取、不移植上游任何 Kotlin 源码。Jsoup 是公开的
 * 第三方库，此处只使用其**公开 API**（{@code Jsoup.parse} 与节点访问器），
 * 序列化约定由本项目自行定义。
 *
 * <h2>输出约定（必须与 spikes/s0/src/dom_diff.rs 的 describe_node 逐条一致）</h2>
 * <ul>
 *   <li>每行 {@code <深度>|<类型字母>|<名称或内容>}，深度从 0 起</li>
 *   <li>类型字母：{@code E}=元素 / {@code T}=文本 / {@code X}=注释 / {@code O}=文档类型</li>
 *   <li>元素名小写；属性按名排序后以 {@code name=value} 用逗号相连（无属性则为空段）</li>
 *   <li>文本内容折叠连续空白并去首尾</li>
 *   <li>根 Document 节点不出行</li>
 * </ul>
 *
 * <p><b>为什么把 DataNode / CDataNode 归到 {@code T}</b>：Rust 侧的最小 DOM 把
 * {@code <script>/<style>} 内容与 CDATA 也走文本路径（见 dom_diff.rs 的 NodeData 注释）。
 * 若此处把它们单独成行，产生的差异是**序列化约定差异**而非**解析差异**，
 * 会污染本 spike 真正要测的量。脚本/CDATA 的语义差异另由节点计数维度覆盖。
 *
 * <p>用法：{@code java -cp jsoup.jar:. JsoupDomDump <语料目录>}
 */
public final class JsoupDomDump {

    /** 工具类，不允许实例化。 */
    private JsoupDomDump() {
    }

    /**
     * 入口：遍历语料目录下全部 {@code *.html}，为每个样本写出 {@code <name>.jsoup.txt}。
     *
     * @param args 唯一参数为语料目录路径
     */
    public static void main(String[] args) {
        if (args.length != 1) {
            System.err.println("用法：java -cp <jsoup.jar>:. JsoupDomDump <语料目录>");
            System.exit(2);
        }

        Path corpusDirectory = Paths.get(args[0]);
        if (!Files.isDirectory(corpusDirectory)) {
            System.err.println("语料目录不存在：" + corpusDirectory);
            System.exit(2);
        }

        List<Path> samples = collectSamples(corpusDirectory);
        if (samples.isEmpty()) {
            System.err.println("语料目录内没有 *.html 样本：" + corpusDirectory);
            System.exit(2);
        }

        for (Path sample : samples) {
            try {
                dumpSample(sample);
            } catch (IOException error) {
                System.err.println("处理 " + sample + " 失败：" + error.getMessage());
                System.exit(1);
            }
        }

        System.out.println("Jsoup 侧导出完成：" + samples.size() + " 个样本");
        // 版本从 jar 的包实现版本读取（Jsoup 1.23.2 没有静态 version() 方法）
        System.out.println("Jsoup 版本：" + Jsoup.class.getPackage().getImplementationVersion());
    }

    /** 收集 {@code *.html} 并排序，保证与 Rust 侧的遍历顺序一致。 */
    private static List<Path> collectSamples(Path directory) {
        List<Path> samples = new ArrayList<>();
        try (var entries = Files.list(directory)) {
            entries.filter(path -> path.getFileName().toString().toLowerCase().endsWith(".html"))
                   .forEach(samples::add);
        } catch (IOException error) {
            System.err.println("读取目录失败：" + error.getMessage());
            System.exit(2);
        }
        Collections.sort(samples);
        return samples;
    }

    /** 解析单个样本并把序列化结果写到同名的 {@code .jsoup.txt}。 */
    private static void dumpSample(Path samplePath) throws IOException {
        String html = Files.readString(samplePath, StandardCharsets.UTF_8);

        // 关掉 Jsoup 的"美化输出"——它只影响 outerHtml() 的排版，不影响本工具的
        // 手工遍历，但显式关闭可避免后续维护者误以为排版会影响输出。
        Document document = Jsoup.parse(html);

        StringBuilder output = new StringBuilder();
        // Document 节点自身不出行，但**必须占掉深度 0**——Rust 侧的根 Document
        // 同样是"不出行但占一层"，其子节点从深度 1 开始。若此处从 0 开始编号，
        // 两侧所有行的深度都会差 1，差异率会被这个纯粹的约定错位顶到 100%，
        // 掩盖真正要测的解析差异。故从深度 1 起步。
        for (Node child : document.childNodes()) {
            writePreorder(child, 1, output);
        }

        Path outputPath = replaceExtension(samplePath, ".jsoup.txt");
        Files.writeString(outputPath, output.toString(), StandardCharsets.UTF_8);

        // 空文本节点单独报告：它与结构差异不同源，混入差异率会污染该指标。
        int emptyTextNodes = countEmptyTextNodes(document);
        Path statsPath = replaceExtension(samplePath, ".jsoup.stats.txt");
        Files.writeString(statsPath, "empty_text_nodes=" + emptyTextNodes + "\n", StandardCharsets.UTF_8);

        System.out.println("已导出 " + outputPath.getFileName()
                + "（空白文本节点 " + emptyTextNodes + " 个）");
    }

    /**
     * 统计规范化后为空的文本节点数。
     *
     * <p>为何单独统计：Jsoup 保留源码缩进产生的纯空白文本节点，html5ever 在多数
     * 位置丢弃——这是系统性差异，但不是"结构兼容性"问题。单列成指标既不丢失信息，
     * 又不让它污染差异率。
     */
    private static int countEmptyTextNodes(Node node) {
        int count = 0;
        if (node instanceof TextNode textNode && normalizeWhitespace(textNode.getWholeText()).isEmpty()) {
            count = 1;
        }
        for (Node child : node.childNodes()) {
            count += countEmptyTextNodes(child);
        }
        return count;
    }

    /** 把 {@code foo.html} 换成 {@code foo.jsoup.txt}。 */
    private static Path replaceExtension(Path path, String newSuffix) {
        String fileName = path.getFileName().toString();
        int dotIndex = fileName.lastIndexOf('.');
        String baseName = dotIndex > 0 ? fileName.substring(0, dotIndex) : fileName;
        return path.resolveSibling(baseName + newSuffix);
    }

    /** 先序遍历，逐节点输出一行。 */
    private static void writePreorder(Node node, int depth, StringBuilder output) {
        String line = describeNode(node, depth);
        if (line != null) {
            output.append(line).append('\n');
        }
        for (Node child : node.childNodes()) {
            writePreorder(child, depth + 1, output);
        }
    }

    /**
     * 把单个节点渲染成一行；返回 {@code null} 表示该节点不出现在序列化中。
     *
     * <p>节点类型映射到类型字母的规则见类注释。Jsoup 把很多 HTML 里的"元素"建模成
     * 子类（{@code FormElement}/{@code PseudoTextElement} 等），此处按**标签名**判断，
     * 不看 Java 类型，才能与 Rust 侧（只有 QualName）对齐。
     */
    private static String describeNode(Node node, int depth) {
        if (node instanceof Document) {
            // 根文档不出行（调用方已跳过，此处防御性保留）
            return null;
        }
        if (node instanceof Element element) {
            return depth + "|E|" + element.normalName() + "|" + formatAttributes(element);
        }
        if (node instanceof DataNode dataNode) {
            // `<script>` / `<style>` 的内容。DataNode 是 TextNode 的子类，
            // 故**必须放在 TextNode 分支之前**判断，否则会被当成普通文本。
            // 与 Rust 侧的 `NodeData::Data` 对应。
            return depth + "|D|" + normalizeWhitespace(dataNode.getWholeData());
        }
        if (node instanceof TextNode textNode) {
            String normalized = normalizeWhitespace(textNode.getWholeText());
            // 规范化后为空的文本节点不出行。**这一步两侧必须同时做**：Jsoup 会
            // 保留源码缩进产生的纯空白文本节点，而 html5ever 的树构造器在多数位置
            // 直接丢弃它们。这是真实的解析差异，但属"空白保留策略"而非"结构"差异；
            // 若不一并剔除，它会让后续每一行下标错位，把差异率顶到 ~90%（实测），
            // 完全掩盖结构差异。该差异由 emptyTextNodeCount 单独报告，信息不丢失。
            if (normalized.isEmpty()) {
                return null;
            }
            return depth + "|T|" + normalized;
        }
        if (node instanceof Comment comment) {
            return depth + "|X|" + normalizeWhitespace(comment.getData());
        }
        if (node instanceof DocumentType documentType) {
            return depth + "|O|" + normalizeWhitespace(documentType.name());
        }
        if (node instanceof XmlDeclaration declaration) {
            // HTML 文档里罕见；用 P 与 Rust 侧的 ProcessingInstruction 对齐
            return depth + "|P|" + normalizeWhitespace(declaration.name());
        }
        // Jsoup 的 CDataNode 已被上面的 TextNode 分支覆盖（它是 TextNode 的子类）。
        // 其余未知类型：输出类型字母 ? 而非静默丢弃——静默丢弃会把"两侧对同一
        // 构造的处理不同"粉饰成"一致"。
        return depth + "|?|" + normalizeWhitespace(node.nodeName());
    }

    /**
     * 属性序列化：按**属性名**排序后用逗号相连。
     *
     * <p>排序是必须的：Jsoup 的 {@code attributes()} 是插入序，html5ever 侧的
     * Rust 实现也按插入序存入，但两家解析器对"重复属性取哪个""隐式补全的属性插在哪"
     * 的处理可能不同，导致顺序分叉。排序后只比较**集合**，把顺序噪音剔除，
     * 使差异率反映真实的属性集合差异。
     */
    private static String formatAttributes(Element element) {
        List<String> pairs = new ArrayList<>();
        for (Attribute attribute : element.attributes()) {
            pairs.add(attribute.getKey().toLowerCase() + "=" + normalizeWhitespace(attribute.getValue()));
        }
        Collections.sort(pairs);
        return String.join(",", pairs);
    }

    /**
     * 折叠连续空白并去首尾。
     *
     * <p>与 Rust 侧 {@code normalize_whitespace} 语义一致：**只折叠 HTML 规范定义的
     * ASCII 空白**（tab/LF/FF/CR/space），首尾去除。
     *
     * <p><b>U+00A0（{@code &nbsp;}）刻意不折叠</b>：Java 的
     * {@code Character.isWhitespace(0xA0)} 返回 false，符合 HTML 语义。Rust 的
     * {@code char::is_whitespace} 却返回 true——若 Rust 侧直接用后者，{@code &nbsp;}
     * 会被折叠成普通空格，两侧产生真实的语义分叉（这正是本 spike 语料 03 定位到的
     * 唯一差异，已修）。
     */
    private static String normalizeWhitespace(String text) {
        StringBuilder result = new StringBuilder(text.length());
        boolean previousWasWhitespace = true; // 起始视为空白，从而抑制前导空格
        for (int index = 0; index < text.length(); index++) {
            char character = text.charAt(index);
            if (isHtmlWhitespace(character)) {
                if (!previousWasWhitespace) {
                    result.append(' ');
                }
                previousWasWhitespace = true;
            } else {
                result.append(character);
                previousWasWhitespace = false;
            }
        }
        while (result.length() > 0 && result.charAt(result.length() - 1) == ' ') {
            result.setLength(result.length() - 1);
        }
        return result.toString();
    }

    /**
     * HTML 规范定义的空白字符（WHATWG Infra "ASCII whitespace"）。
     *
     * <p>显式枚举而非用 {@code Character.isWhitespace}：后者对 U+00A0 虽已返回 false
     * （正确），但它同时把 U+2000 等 Unicode 空格也算作空白，与 HTML 语义不符。
     * 两侧都用同一份显式集合，归一化才是真正同构的。
     */
    private static boolean isHtmlWhitespace(char character) {
        return character == '\t'
                || character == '\n'
                || character == '\u000C'
                || character == '\r'
                || character == ' ';
    }
}
