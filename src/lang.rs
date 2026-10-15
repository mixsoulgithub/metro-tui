//! 界面语言.只管应用自己的提示文字:底部状态栏、键位速查、选城市界面、":" 命令回显.
//! 命令行那一套(-h 帮助与参数报错)固定英文,不跟着它走.

/// 界面语言(默认中文)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Zh,
    En,
}

impl Default for Lang {
    fn default() -> Self {
        Lang::Zh
    }
}

impl Lang {
    /// 取当前语言的那一条(静态文案用)
    pub fn pick<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        match self {
            Lang::Zh => zh,
            Lang::En => en,
        }
    }

    /// 取当前语言的那一条(要拼字符串时用;两条都会求值,反正是提示文字)
    pub fn pick_owned(&self, zh: String, en: String) -> String {
        match self {
            Lang::Zh => zh,
            Lang::En => en,
        }
    }

    /// 另一种语言(不带参数的 :lang 就切到它)
    pub fn other(&self) -> Lang {
        match self {
            Lang::Zh => Lang::En,
            Lang::En => Lang::Zh,
        }
    }

    /// 短代码,提示里用
    pub fn code(&self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    /// 解析命令行 / :lang 的参数
    pub fn parse(s: &str) -> Option<Lang> {
        match s.trim().to_ascii_lowercase().as_str() {
            "zh" | "cn" | "zh-cn" | "chinese" | "中文" => Some(Lang::Zh),
            "en" | "en-us" | "english" => Some(Lang::En),
            _ => None,
        }
    }
}
