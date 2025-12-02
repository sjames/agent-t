use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Arguments for the MathCalc tool
#[derive(Debug, Deserialize)]
pub struct MathCalcArgs {
    /// The mathematical expression to evaluate
    pub expression: String,
}

/// Tool to evaluate mathematical expressions
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct MathCalc;

impl Tool for MathCalc {
    const NAME: &'static str = "math_calc";
    type Error = ToolError;
    type Args = MathCalcArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Evaluate mathematical expressions. Supports basic arithmetic (+, -, *, /), exponentiation (^), parentheses, and common mathematical functions (sin, cos, tan, sqrt, log, ln, abs, etc.). Use this tool for any mathematical calculations.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "The mathematical expression to evaluate (e.g., '2 + 2', 'sqrt(16)', 'sin(3.14159/2)', '2^10')"
                    }
                },
                "required": ["expression"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Evaluate the expression using sci-calc
        let mut ctx = sci_calc::context::Context::new();
        match sci_calc::calculate(&args.expression, &mut ctx) {
            Ok(result) => {
                // Format the result nicely
                Ok(format!("{} = {}", args.expression, result))
            }
            Err(e) => {
                // Return the error message
                Err(ToolError::Other(format!("Math evaluation error: {}", e)))
            }
        }
    }
}
